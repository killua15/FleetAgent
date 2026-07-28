//! Real PR creation + review-comment polling via the `gh` CLI — Hangar has no
//! GitHub App/webhook (it's a desktop app with no public endpoint), so this
//! is the same shape Conductor uses: poll while the app is open, plus a
//! manual "check now" escape hatch. Never auto-sends new comments to an
//! agent on its own — `check_pr_comments`/the poller only ever *surface*
//! them; `send_pr_comments_to_session` is a separate, explicit action the
//! user triggers from the UI.

use crate::commands::sessions::{ensure_worktree_dir_ignored, slugify, unique_worktree_dir};
use crate::db::DbConnection;
use crate::models::{PrComment, PullRequestInfo, PullRequestPreview};
use crate::providers;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

pub(crate) fn git_capture(cwd: &Path, args: &[&str]) -> Result<String, String> {
    git_capture_with_env(cwd, args, &[])
}

/// `git_capture` with extra environment — the project's identity and push
/// credential (see `git_identity`). Separate rather than a default because most
/// git calls here are reads that don't need either, and threading a keychain
/// lookup through `rev-parse` would be a cost paid on every call for nothing.
pub(crate) fn git_capture_with_env(cwd: &Path, args: &[&str], env: &[(String, String)]) -> Result<String, String> {
    let mut cmd = Command::new(providers::resolve_binary("git"));
    cmd.args(args).current_dir(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = crate::process_timeout::run_with_timeout(cmd, crate::process_timeout::DEFAULT_TIMEOUT)
        .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Every `gh` call in this file goes through here — a hung `gh` process
/// (dead network, an interactive re-auth prompt) would otherwise block
/// whichever `run_blocking` thread called it forever, see
/// `process_timeout` for why that matters.
pub(crate) fn gh_output(cwd: Option<&Path>, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new(providers::resolve_binary("gh"));
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    crate::process_timeout::run_with_timeout(cmd, crate::process_timeout::DEFAULT_TIMEOUT).map_err(|e| format!("gh {}: {e}", args.join(" ")))
}

/// Clear, actionable error instead of letting a bare `gh pr create` failure
/// (missing binary or expired login) surface as a cryptic exit-code message.
pub(crate) fn gh_available_and_authed() -> Result<(), String> {
    let gh_path = providers::resolve_binary("gh");
    if gh_path == "gh" {
        return Err("the GitHub CLI (gh) isn't installed — install it from https://cli.github.com and run `gh auth login`".to_string());
    }
    let output = gh_output(None, &["auth", "status"])?;
    if !output.status.success() {
        return Err("gh isn't logged in — run `gh auth login` first".to_string());
    }
    Ok(())
}

/// `origin/HEAD`'s target if the remote's default branch was ever resolved
/// locally, else `gh repo view`, else a plain `"main"` guess — always
/// returns something rather than failing, since every caller has a
/// reasonable fallback path anyway.
pub(crate) fn detect_default_branch(cwd: &Path) -> String {
    if let Ok(full) = git_capture(cwd, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]) {
        if let Some(name) = full.strip_prefix("origin/") {
            return name.to_string();
        }
    }
    if let Ok(output) = gh_output(Some(cwd), &["repo", "view", "--json", "defaultBranchRef", "-q", ".defaultBranchRef.name"]) {
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !name.is_empty() {
                return name;
            }
        }
    }
    "main".to_string()
}

/// Whether a local branch ref by this name exists *anywhere* in the repo —
/// checked out or not. A plain existence check (not a "checked out
/// elsewhere" check) is deliberately the more conservative question: if the
/// ref exists at all, `resolve_session_for_pr_branch` leaves it alone
/// rather than guessing whether it's safe to reuse or move.
pub(crate) fn local_branch_exists(cwd: &Path, branch: &str) -> bool {
    Command::new(providers::resolve_binary("git"))
        .args(["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")])
        .current_dir(cwd)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(crate) fn parse_owner_repo_from_url(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim().trim_end_matches(".git");
    let idx = trimmed.find("github.com")?;
    let after_host = &trimmed[idx + "github.com".len()..];
    let cleaned = after_host.trim_start_matches(':').trim_start_matches('/');
    let mut parts = cleaned.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    (!owner.is_empty() && !repo.is_empty()).then_some((owner, repo))
}

pub(crate) fn detect_repo_owner_and_name(cwd: &Path) -> Result<(String, String), String> {
    let url = git_capture(cwd, &["remote", "get-url", "origin"])?;
    parse_owner_repo_from_url(&url).ok_or_else(|| format!("could not parse an owner/repo out of remote url: {url}"))
}

/// Splits `git diff --name-status` output into (added, modified, removed) —
/// pure so it's testable without a real repo. Renames/copies (`R`/`C`) are
/// folded into "modified" rather than getting their own bucket, matching
/// the mockup's three groups.
fn group_name_status(name_status: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let (mut added, mut modified, mut removed) = (Vec::new(), Vec::new(), Vec::new());
    for line in name_status.lines() {
        let mut fields = line.split('\t');
        let status = fields.next().unwrap_or("");
        // Renames/copies (`R100\told\tnew`) carry both the old and new
        // path — the new path (the last field) is the one worth showing.
        let path = fields.last().unwrap_or("").trim().to_string();
        if path.is_empty() {
            continue;
        }
        match status.chars().next() {
            Some('A') => added.push(path),
            Some('D') => removed.push(path),
            _ => modified.push(path),
        }
    }
    (added, modified, removed)
}

fn render_pr_body(added: &[String], modified: &[String], removed: &[String]) -> String {
    let mut body = String::new();
    let mut section = |title: &str, paths: &[String]| {
        if paths.is_empty() {
            return;
        }
        body.push_str(&format!("### {title}\n"));
        for p in paths {
            body.push_str(&format!("- {p}\n"));
        }
        body.push('\n');
    };
    section("Added", added);
    section("Modified", modified);
    section("Removed", removed);
    body.push_str("_Opened by Hangar._");
    body
}

fn generate_pr_title_and_body(cwd: &Path, base: &str, branch: &str, session_name: &str) -> Result<(String, String), String> {
    let name_status = git_capture(cwd, &["diff", "--name-status", &format!("{base}...{branch}")])?;
    let (added, modified, removed) = group_name_status(&name_status);
    Ok((session_name.to_string(), render_pr_body(&added, &modified, &removed)))
}

/// The columns `row_to_pull_request` reads, in its exact order. One constant
/// rather than the same list spelled out at five call sites, where adding a
/// column meant finding every one of them or getting a silent off-by-one.
pub(crate) const PR_COLUMNS: &str =
    "id, project_id, session_id, repo_owner, repo_name, number, url, state, last_comment_seen_at, created_at, last_check_error, last_seen_updated_at, head_branch, base_branch, title, author_login, relation";

/// `PR_COLUMNS` qualified with a table alias, for the queries that join.
pub(crate) fn pr_columns_from(alias: &str) -> String {
    PR_COLUMNS.split(", ").map(|c| format!("{alias}.{c}")).collect::<Vec<_>>().join(", ")
}

pub(crate) fn row_to_pull_request(row: &rusqlite::Row) -> rusqlite::Result<PullRequestInfo> {
    Ok(PullRequestInfo {
        id: row.get(0)?,
        project_id: row.get(1)?,
        session_id: row.get(2)?,
        repo_owner: row.get(3)?,
        repo_name: row.get(4)?,
        number: row.get(5)?,
        url: row.get(6)?,
        state: row.get(7)?,
        last_comment_seen_at: row.get(8)?,
        created_at: row.get(9)?,
        last_check_error: row.get(10)?,
        last_seen_updated_at: row.get(11)?,
        head_branch: row.get(12)?,
        base_branch: row.get(13)?,
        title: row.get(14)?,
        author_login: row.get(15)?,
        relation: row.get(16)?,
    })
}

/// The most recent PR for a session — correct only when a session has at
/// most one PR, which is true of every PR Hangar's own "Create PR" button
/// makes, but not a rule the schema enforces. Discovered PRs are no longer
/// hung off an arbitrary session (they carry their project instead), so this
/// no longer risks collapsing a project's whole PR list into one row — but
/// anything that must reach a *specific* PR should still key on the PR's own
/// id via `fetch_pull_request_by_id`.
fn fetch_pull_request(db: &DbConnection, session_id: &str) -> Result<Option<PullRequestInfo>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        &format!("SELECT {PR_COLUMNS} FROM pull_requests WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 1"),
        params![session_id],
        row_to_pull_request,
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn fetch_pull_request_by_id(db: &DbConnection, pr_id: &str) -> Result<Option<PullRequestInfo>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(&format!("SELECT {PR_COLUMNS} FROM pull_requests WHERE id = ?1"), params![pr_id], row_to_pull_request)
        .optional()
        .map_err(|e| e.to_string())
}

/// Ensures `cwd` is on a pushed, PR-ready branch: if it's currently sitting
/// on the base branch (or not on any real branch at all — a detached HEAD,
/// which `git rev-parse --abbrev-ref HEAD` reports as the literal string
/// `"HEAD"` and which `git push` can't publish under that name), creates
/// and checks out a new one (`hangar/<slug>`); commits anything uncommitted
/// (no stage/commit UI exists yet, so this is a single blanket commit);
/// pushes to `origin`. Returns `(branch, base)`. Pure git, no `gh` calls, so
/// it's testable against a real local remote without touching GitHub.
fn prepare_branch_for_pr(cwd: &Path, session_name: &str, git_env: &[(String, String)]) -> Result<(String, String), String> {
    let base = detect_default_branch(cwd);
    let current_branch = git_capture(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = if current_branch == base || current_branch == "HEAD" {
        let new_branch = format!("hangar/{}", slugify(session_name));
        git_capture(cwd, &["checkout", "-b", &new_branch])?;
        new_branch
    } else {
        current_branch
    };

    let status = git_capture(cwd, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        git_capture(cwd, &["add", "-A"])?;
        // The two calls that must not use the machine's default identity: a
        // commit records who wrote it, forever, and a push has to authenticate
        // as whoever owns that repo.
        git_capture_with_env(cwd, &["commit", "-m", &format!("Changes from Hangar session: {session_name}")], git_env)?;
    }

    git_capture_with_env(cwd, &["push", "-u", "origin", &branch], git_env)?;
    Ok((branch, base))
}

/// A session's working directory (its own worktree if it has one, else the
/// project's own path) plus its name — the one DB lookup both
/// `create_pull_request` and `preview_pull_request` need before they can
/// touch git at all.
fn resolve_session_cwd_and_name(db: &DbConnection, session_id: &str) -> Result<(String, String), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let (project_id, worktree_path, name): (String, Option<String>, String) = conn
        .query_row(
            "SELECT project_id, worktree_path, name FROM sessions WHERE id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("session lookup failed: {e}"))?;
    let cwd = match worktree_path {
        Some(p) => p,
        None => conn
            .query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
            .map_err(|e| format!("project lookup failed: {e}"))?,
    };
    Ok((cwd, name))
}

/// Read-only preview of what `create_pull_request` would produce, without
/// any of its mutations (no branch creation, no commit, no push) — powers
/// the "Create pull request" dialog's title/body/diffstat preview.
///
/// Diffs the working tree against `git merge-base <base> HEAD` rather than
/// requiring a pushed feature branch to already exist: for a session
/// already on one, this captures every commit since it diverged from base
/// plus anything still uncommitted; for a session still sitting on the
/// base branch itself, the merge-base is just its own HEAD, so this
/// naturally reduces to "just the uncommitted changes" — one code path
/// covers both states `create_pull_request` has to handle, and needs no
/// `gh` call at all (so it works even without `gh` installed/authed).
#[tauri::command]
pub async fn preview_pull_request<R: Runtime>(app: AppHandle<R>, session_id: String) -> Result<PullRequestPreview, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (cwd, session_name) = resolve_session_cwd_and_name(&db, &session_id)?;
        let cwd_path = Path::new(&cwd);
        if !cwd_path.is_dir() {
            return Err(format!("working directory {cwd} does not exist"));
        }

        let base = detect_default_branch(cwd_path);
        let merge_base = git_capture(cwd_path, &["merge-base", &base, "HEAD"]).map_err(|e| format!("could not diff against {base}: {e}"))?;

        let name_status = git_capture(cwd_path, &["diff", "--name-status", &merge_base])?;
        let (added, modified, removed) = group_name_status(&name_status);

        let numstat = git_capture(cwd_path, &["diff", "--numstat", &merge_base])?;
        let (mut lines_added, mut lines_removed) = (0i64, 0i64);
        for line in numstat.lines() {
            let mut fields = line.split('\t');
            // Binary files report `-` instead of a number for both columns
            // — skip them rather than letting the running total error out.
            if let (Some(a), Some(r)) = (fields.next(), fields.next()) {
                lines_added += a.parse().unwrap_or(0);
                lines_removed += r.parse().unwrap_or(0);
            }
        }

        let body = render_pr_body(&added, &modified, &removed);
        Ok(PullRequestPreview { suggested_title: session_name, body, added, modified, removed, lines_added, lines_removed })
    })
    .await?
}

/// Opens a real PR for `session_id`: pushes the branch (creating one first
/// if the session is still sitting on the base branch), commits whatever's
/// uncommitted (no stage/commit UI exists yet — everything goes in one
/// commit), and runs `gh pr create`. Works for both subsessions (their own
/// worktree/branch) and top-level sessions (the project's own directory,
/// possibly still on the base branch). `title` overrides the generated
/// default (the session's own name) when the user edited it in the
/// preview dialog; `None` keeps existing callers' behavior unchanged.
#[tauri::command]
pub async fn create_pull_request<R: Runtime>(app: AppHandle<R>, session_id: String, title: Option<String>) -> Result<PullRequestInfo, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        gh_available_and_authed()?;

        let (cwd, session_name) = resolve_session_cwd_and_name(&db, &session_id)?;
        let cwd_path = Path::new(&cwd);
        if !cwd_path.is_dir() {
            return Err(format!("working directory {cwd} does not exist"));
        }

        // The project's own identity and credential, if it has one — otherwise
        // the machine's default, which is the pre-existing behaviour.
        let project_id_for_git: String = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT project_id FROM sessions WHERE id = ?1", params![session_id], |row| row.get(0))
                .map_err(|e| format!("session lookup failed: {e}"))?
        };
        let git_env = crate::git_identity::git_env_for_project(&app, &project_id_for_git);
        let (branch, base) = prepare_branch_for_pr(cwd_path, &session_name, &git_env)?;

        let (owner, repo) = detect_repo_owner_and_name(cwd_path)?;
        let (default_title, body) = generate_pr_title_and_body(cwd_path, &base, &branch, &session_name)?;
        let title = title.unwrap_or(default_title);

        let output = gh_output(Some(cwd_path), &["pr", "create", "--title", &title, "--body", &body, "--base", &base, "--head", &branch])?;
        if !output.status.success() {
            return Err(format!("gh pr create failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let number: i64 = url.rsplit('/').next().and_then(|s| s.parse().ok()).ok_or_else(|| format!("could not parse a PR number out of: {url}"))?;

        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let project_id: String = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let project_id: String = conn
                .query_row("SELECT project_id FROM sessions WHERE id = ?1", params![session_id], |row| row.get(0))
                .map_err(|e| format!("session lookup failed: {e}"))?;
            conn.execute(
                "INSERT INTO pull_requests (id, project_id, session_id, repo_owner, repo_name, number, url, state, last_comment_seen_at, created_at, head_branch, base_branch, title, relation)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'open', NULL, ?8, ?9, ?10, ?11, 'mine')",
                params![id, project_id, session_id, owner, repo, number, url, now, branch, base, title],
            )
            .map_err(|e| e.to_string())?;
            project_id
        };
        Ok(PullRequestInfo {
            id,
            project_id,
            session_id: Some(session_id),
            repo_owner: owner,
            repo_name: repo,
            number,
            url,
            state: "open".to_string(),
            last_comment_seen_at: None,
            last_check_error: None,
            created_at: now,
            last_seen_updated_at: None,
            head_branch: Some(branch),
            base_branch: Some(base),
            title: Some(title),
            // Hangar just opened it as the `gh`-authenticated user, so there's
            // nothing to discover: it's mine by construction.
            author_login: None,
            relation: "mine".to_string(),
        })
    })
    .await?
}

#[tauri::command]
pub fn get_pull_request(db: State<DbConnection>, session_id: String) -> Result<Option<PullRequestInfo>, String> {
    fetch_pull_request(&db, &session_id)
}

/// Same shape as `PrComment` minus `id` — nothing has a real
/// `pr_review_comments` row yet at parse time, that only happens once we've
/// decided which of these are actually new and not our own. `external_id` is
/// GitHub's own id for the comment/review (a GraphQL node id for
/// `comments`/`reviews`, a plain REST id for inline diff comments) — the
/// actual "have we already seen this one" key. Not a timestamp: a PR's
/// history can grow a source of comments Hangar couldn't previously fetch
/// (inline diff comments were invisible before this was added) — if
/// "already surfaced" were tracked as "newer than the latest timestamp seen
/// so far", any such backfilled source would stay permanently invisible the
/// moment a single newer comment from any other source had already advanced
/// that watermark. Identity-based dedup has no such blind spot: a comment
/// is new exactly when Hangar has never persisted its id before.
#[derive(Debug, Clone, PartialEq)]
struct RawPrComment {
    external_id: String,
    author: String,
    body: String,
    created_at: String,
    path: Option<String>,
    line: Option<i64>,
}

/// The PR fields every GraphQL query here asks for, as a fragment shared by
/// the repo-wide and single-PR queries so the two can never drift.
///
/// **The `id` choices are a compatibility contract, not a preference.**
/// `external_id` values already in the database came from `gh pr view --json
/// comments,reviews` (GraphQL node ids like `PRR_kwDO…`, verified identical to
/// what `id` returns here) and from the REST inline-comments endpoint (numeric
/// ids). So: `id` for comments and reviews, `databaseId` for review-thread
/// comments. Getting this wrong wouldn't fail loudly — it would make every
/// comment ever seen look new and flood the inbox on the first pass.
const PR_FIELDS_FRAGMENT: &str = r#"
fragment prFields on PullRequest {
  number url title createdAt updatedAt state
  author { login }
  assignees(first: 10) { nodes { login } }
  headRefName baseRefName
  comments(last: 50) { nodes { id body createdAt author { login } } }
  reviews(last: 50) { nodes { id body submittedAt author { login } } }
  reviewThreads(last: 50) { nodes { comments(first: 20) { nodes { databaseId body createdAt path line originalLine author { login } } } } }
}"#;

/// Everything one poll pass needs about one repo, in a single round trip: the
/// open PRs with their comments, who the viewer is, and which PRs GitHub
/// itself says are waiting on the viewer's review.
///
/// This replaces a fan-out that cost, per pass: two `gh` calls per repo plus
/// one to three per tracked PR (a state probe, a comments fetch, and a REST
/// call for inline comments), each a process spawn of ~0.5-0.9s. Measured on
/// this machine: 11-21 processes and 10-20 seconds became one 0.4-0.8s call per
/// repo. `viewer { login }` also folds in what used to be a separate
/// `gh api user` per PR.
#[derive(Debug, Clone, PartialEq)]
struct RepoPrSnapshot {
    /// The `gh`-authenticated user, or `None` when GitHub didn't say — see
    /// `classify_pr` for why that isn't treated as "everything is mine".
    viewer_login: Option<String>,
    review_requested: HashSet<i64>,
    prs: Vec<SnapshotPr>,
}

/// One open PR as GitHub returned it, with its comments already normalized.
#[derive(Debug, Clone, PartialEq)]
struct SnapshotPr {
    number: i64,
    url: String,
    title: Option<String>,
    created_at: String,
    updated_at: String,
    /// `open` / `closed` / `merged`, lowercased to match the `state` column.
    state: String,
    author_login: Option<String>,
    assignees: Vec<String>,
    head_branch: Option<String>,
    base_branch: Option<String>,
    comments: Vec<RawPrComment>,
}

/// What `check_pr_comments_with` needs to judge one PR, from either source:
/// a repo-wide snapshot (the poller) or a single-PR query (the per-session
/// "check now"). Splitting this out is what lets one repo query serve every
/// tracked PR in it instead of one query each.
#[derive(Debug, Clone, PartialEq)]
struct PrFacts {
    state: String,
    updated_at: String,
    comments: Vec<RawPrComment>,
    viewer_login: Option<String>,
}

/// Pulls the three comment sources out of one PR's GraphQL node and merges
/// them into one list, oldest first.
///
/// Same rules as the two REST parsers this replaces: bodyless reviews (a bare
/// "approve" with nothing to act on) are dropped, and an inline comment's
/// `line` falls back to `originalLine`, which is what GitHub leaves once the
/// commented line has scrolled out of the current diff.
fn parse_graphql_pr_comments(pr: &Value) -> Vec<RawPrComment> {
    let mut out = Vec::new();

    let mut push_simple = |nodes: Option<&Vec<Value>>, date_field: &str| {
        for c in nodes.into_iter().flatten() {
            let body = c.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if body.trim().is_empty() {
                continue;
            }
            out.push(RawPrComment {
                external_id: c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                author: c.pointer("/author/login").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                body,
                created_at: c.get(date_field).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                path: None,
                line: None,
            });
        }
    };
    push_simple(pr.pointer("/comments/nodes").and_then(|v| v.as_array()), "createdAt");
    push_simple(pr.pointer("/reviews/nodes").and_then(|v| v.as_array()), "submittedAt");

    for thread in pr.pointer("/reviewThreads/nodes").and_then(|v| v.as_array()).into_iter().flatten() {
        for c in thread.pointer("/comments/nodes").and_then(|v| v.as_array()).into_iter().flatten() {
            let body = c.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if body.trim().is_empty() {
                continue;
            }
            out.push(RawPrComment {
                // `databaseId`, not `id`: this is the source whose ids were
                // persisted as REST numerics.
                external_id: c.get("databaseId").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
                author: c.pointer("/author/login").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                body,
                created_at: c.get("createdAt").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                path: c.get("path").and_then(|v| v.as_str()).map(|s| s.to_string()),
                line: c.get("line").and_then(|v| v.as_i64()).or_else(|| c.get("originalLine").and_then(|v| v.as_i64())),
            });
        }
    }

    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

fn parse_graphql_pr(pr: &Value) -> Option<SnapshotPr> {
    Some(SnapshotPr {
        number: pr.get("number").and_then(|v| v.as_i64())?,
        url: pr.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        title: pr.get("title").and_then(|v| v.as_str()).map(|s| s.to_string()),
        created_at: pr.get("createdAt").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        updated_at: pr.get("updatedAt").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        state: pr.get("state").and_then(|v| v.as_str()).unwrap_or("OPEN").to_lowercase(),
        author_login: pr.pointer("/author/login").and_then(|v| v.as_str()).map(|s| s.to_string()),
        assignees: pr
            .pointer("/assignees/nodes")
            .and_then(|v| v.as_array())
            .map(|list| list.iter().filter_map(|a| a.get("login").and_then(|v| v.as_str()).map(|s| s.to_string())).collect())
            .unwrap_or_default(),
        head_branch: pr.get("headRefName").and_then(|v| v.as_str()).map(|s| s.to_string()),
        base_branch: pr.get("baseRefName").and_then(|v| v.as_str()).map(|s| s.to_string()),
        comments: parse_graphql_pr_comments(pr),
    })
}

/// Parses the repo-wide query's response. Pure, so the whole shape of a poll
/// pass is testable against a fixture instead of a live repo.
fn parse_repo_snapshot(json: &Value) -> RepoPrSnapshot {
    RepoPrSnapshot {
        viewer_login: json.pointer("/data/viewer/login").and_then(|v| v.as_str()).map(|s| s.to_string()),
        review_requested: json
            .pointer("/data/reviewRequested/nodes")
            .and_then(|v| v.as_array())
            .map(|rows| rows.iter().filter_map(|r| r.get("number").and_then(|v| v.as_i64())).collect())
            .unwrap_or_default(),
        prs: json
            .pointer("/data/repository/pullRequests/nodes")
            .and_then(|v| v.as_array())
            .map(|nodes| nodes.iter().filter_map(parse_graphql_pr).collect())
            .unwrap_or_default(),
    }
}

/// One round trip for one repo. `limit` caps the open PRs pulled in — see
/// `DISCOVERY_PR_LIMIT`.
fn fetch_repo_pr_snapshot(owner: &str, repo: &str, limit: usize) -> Result<RepoPrSnapshot, String> {
    let query = format!(
        r#"query($owner:String!, $name:String!, $limit:Int!, $search:String!) {{
  viewer {{ login }}
  repository(owner:$owner, name:$name) {{
    pullRequests(states: OPEN, first: $limit, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{ ...prFields }} }}
  }}
  reviewRequested: search(query: $search, type: ISSUE, first: 100) {{ nodes {{ ... on PullRequest {{ number }} }} }}
}}{PR_FIELDS_FRAGMENT}"#
    );
    let search = format!("repo:{owner}/{repo} is:pr is:open review-requested:@me");
    let limit = limit.to_string();
    let output = gh_output(
        None,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("owner={owner}"),
            "-F",
            &format!("name={repo}"),
            "-F",
            &format!("limit={limit}"),
            "-f",
            &format!("search={search}"),
        ],
    )?;
    if !output.status.success() {
        return Err(format!("gh api graphql failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    Ok(parse_repo_snapshot(&json))
}

/// One PR, for the paths that don't have a repo snapshot to hand: the
/// per-session "check now", and the poller resolving a tracked PR that has
/// dropped off the open list (merged or closed — the repo query only returns
/// open ones, so absence is the signal but not the reason).
fn fetch_single_pr_facts(pr: &PullRequestInfo) -> Result<PrFacts, String> {
    let query = format!(
        r#"query($owner:String!, $name:String!, $number:Int!) {{
  viewer {{ login }}
  repository(owner:$owner, name:$name) {{ pullRequest(number:$number) {{ ...prFields }} }}
}}{PR_FIELDS_FRAGMENT}"#
    );
    let output = gh_output(
        None,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("owner={}", pr.repo_owner),
            "-F",
            &format!("name={}", pr.repo_name),
            "-F",
            &format!("number={}", pr.number),
        ],
    )?;
    if !output.status.success() {
        return Err(format!("gh api graphql failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let node = json.pointer("/data/repository/pullRequest").ok_or_else(|| "gh returned no pull request".to_string())?;
    let parsed = parse_graphql_pr(node).ok_or_else(|| "gh returned a pull request with no number".to_string())?;
    Ok(PrFacts {
        state: parsed.state,
        updated_at: parsed.updated_at,
        comments: parsed.comments,
        viewer_login: json.pointer("/data/viewer/login").and_then(|v| v.as_str()).map(|s| s.to_string()),
    })
}

/// Of everything fetched, keep only comments that are (a) not already
/// persisted for this PR (by GitHub's own id, not a timestamp watermark —
/// see `RawPrComment`'s doc comment for why) and (b) not written by the
/// logged-in `gh` user themselves — pure so both conditions are unit
/// testable without a real `gh` call or database.
fn select_new_comments_from_others(all: Vec<RawPrComment>, already_tracked: &HashSet<String>, self_login: Option<&str>) -> Vec<RawPrComment> {
    all.into_iter()
        .filter(|c| !already_tracked.contains(&c.external_id))
        .filter(|c| self_login != Some(c.author.as_str()))
        .collect()
}

/// What a check should do next, decided purely from the cheap probe's result
/// — pure so it's unit-testable without a real `gh` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckOutcome {
    /// Merged is terminal — nothing left to ever review. Delete the row
    /// (and its comments, via cascade) instead of leaving it around forever.
    Cleanup,
    /// Closed without merging can still be reopened — record the new state
    /// and stop actively fetching, but keep the row and its comments.
    Pause,
    /// Still open, and GitHub's own `updatedAt` hasn't moved since the last
    /// successful check — skip the expensive comment fetch.
    NothingChanged,
    /// Still open and something changed (or this PR has never been
    /// successfully checked before) — worth the full fetch.
    NeedsFullFetch,
}

fn decide_check_outcome(state: &str, updated_at: &str, last_seen_updated_at: Option<&str>) -> CheckOutcome {
    if state == "merged" {
        return CheckOutcome::Cleanup;
    }
    if state != "open" {
        return CheckOutcome::Pause;
    }
    if last_seen_updated_at == Some(updated_at) {
        return CheckOutcome::NothingChanged;
    }
    CheckOutcome::NeedsFullFetch
}

/// Checks one *session's* PR for new comments (the newest one, if a project
/// somehow has several sharing that session — see `fetch_pull_request`'s doc
/// comment). This is the shape the manual per-session "check now" in
/// AgentPanel needs; the poller and the PR review inbox's "check now" need
/// every PR checked, not just one per session — they call
/// `check_pr_comments_for_pr_id` directly instead.
fn check_pr_comments_impl(db: &DbConnection, session_id: &str) -> Result<Vec<PrComment>, String> {
    let pr = fetch_pull_request(db, session_id)?.ok_or_else(|| "no PR linked to this session".to_string())?;
    check_pr_comments_for(db, pr)
}

/// Checks one PR for new comments. A `gh` failure (permissions, SSO, rate
/// limit) is captured into `pull_requests.last_check_error` and surfaced via
/// `list_pr_review_inbox` — NOT propagated as an `Err` here, since that would
/// (a) abort the whole poll/check-all-open-PRs loop's remaining PRs and
/// (b) previously just got swallowed by a bare `eprintln!` in the caller,
/// leaving "no new comments" and "the check is silently failing every time"
/// visually indistinguishable. A successful check clears the field.
///
/// The row's lifetime is tied to GitHub's own state, not left to grow
/// forever: a merged PR is deleted outright (cascading its comments);
/// everything else stays in the database until the user explicitly
/// sends/dismisses a comment, or the PR itself gets merged.
fn check_pr_comments_for(db: &DbConnection, pr: PullRequestInfo) -> Result<Vec<PrComment>, String> {
    let facts = fetch_single_pr_facts(&pr);
    check_pr_comments_with(db, pr, facts)
}

/// `check_pr_comments_for`'s body, with the fetch already done.
///
/// The split is the whole point of the GraphQL move: the poller gets every
/// tracked PR's facts out of **one** query per repo and calls this directly,
/// while the per-session "check now" still fetches its own. Same state machine,
/// same persistence, same error handling either way.
///
/// Note that the comment fetch is no longer separate from the freshness probe —
/// one query returns both, so `NothingChanged` now saves the *parsing and
/// writing*, not a second round trip. The check stays because it's what keeps
/// `last_seen_updated_at` meaningful and avoids pointless writes.
fn check_pr_comments_with(db: &DbConnection, pr: PullRequestInfo, facts: Result<PrFacts, String>) -> Result<Vec<PrComment>, String> {
    let facts = match facts {
        Ok(facts) => facts,
        Err(e) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute("UPDATE pull_requests SET last_check_error = ?2 WHERE id = ?1", params![pr.id, e]).map_err(|e| e.to_string())?;
            return Ok(vec![]);
        }
    };
    let PrFacts { state, updated_at, comments: all_comments, viewer_login: self_login } = facts;

    match decide_check_outcome(&state, &updated_at, pr.last_seen_updated_at.as_deref()) {
        CheckOutcome::Cleanup => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM pull_requests WHERE id = ?1", params![pr.id]).map_err(|e| e.to_string())?;
            return Ok(vec![]);
        }
        CheckOutcome::Pause => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute("UPDATE pull_requests SET state = ?2, last_check_error = NULL WHERE id = ?1", params![pr.id, state]).map_err(|e| e.to_string())?;
            return Ok(vec![]);
        }
        CheckOutcome::NothingChanged => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE pull_requests SET state = 'open', last_comment_seen_at = ?2, last_check_error = NULL WHERE id = ?1",
                params![pr.id, now],
            )
            .map_err(|e| e.to_string())?;
            return Ok(vec![]);
        }
        CheckOutcome::NeedsFullFetch => {}
    }

    let mut persisted = Vec::new();
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let already_tracked: HashSet<String> = {
            let mut stmt = conn
                .prepare("SELECT external_id FROM pr_review_comments WHERE pull_request_id = ?1 AND external_id IS NOT NULL")
                .map_err(|e| e.to_string())?;
            let rows = stmt.query_map(params![pr.id], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
            rows.filter_map(Result::ok).collect()
        };
        // Opt-in escape hatch for testing solo: normally a comment from the
        // logged-in `gh` user themselves is dropped (the feature exists to
        // surface *other* reviewers' feedback), but with nobody else around
        // to comment, that makes the whole flow untestable — `None` here
        // reuses `select_new_comments_from_others`'s existing "no filtering"
        // behavior rather than needing a separate code path.
        let include_own = crate::commands::settings::read_app_settings(&conn)?.include_own_pr_comments;
        let effective_self_login = if include_own { None } else { self_login.as_deref() };
        let new_from_others = select_new_comments_from_others(all_comments, &already_tracked, effective_self_login);

        let now = Utc::now().to_rfc3339();
        for raw in new_from_others {
            let id = Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO pr_review_comments (id, pull_request_id, author, body, comment_created_at, path, line, external_id, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![id, pr.id, raw.author, raw.body, raw.created_at, raw.path, raw.line, raw.external_id, now],
            )
            .map_err(|e| e.to_string())?;
            persisted.push(PrComment { id, author: raw.author, body: raw.body, created_at: raw.created_at, path: raw.path, line: raw.line });
        }
        // `last_comment_seen_at` is now just "last time this PR was
        // successfully checked" — dedup no longer depends on it.
        conn.execute(
            "UPDATE pull_requests SET state = 'open', last_comment_seen_at = ?2, last_check_error = NULL, last_seen_updated_at = ?3 WHERE id = ?1",
            params![pr.id, now, updated_at],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(persisted)
}

/// Manual "check now" — same logic the background poller uses per-PR.
#[tauri::command]
pub async fn check_pr_comments(app: AppHandle, session_id: String) -> Result<Vec<PrComment>, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        check_pr_comments_impl(&db, &session_id)
    })
    .await?
}

fn format_pr_comments_prompt(comments: &[PrComment]) -> String {
    let mut out = String::from("PR review feedback:\n\n");
    for c in comments {
        match (&c.path, c.line) {
            (Some(path), Some(line)) => out.push_str(&format!("- {} ({path}:{line}): {}\n", c.author, c.body)),
            _ => out.push_str(&format!("- {}: {}\n", c.author, c.body)),
        }
    }
    out.push_str("\nPlease address these.");
    out
}

fn fetch_comments_by_ids(db: &DbConnection, ids: &[String]) -> Result<Vec<PrComment>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for id in ids {
        if let Some(c) = conn
            .query_row("SELECT id, author, body, comment_created_at, path, line FROM pr_review_comments WHERE id = ?1", params![id], |row| {
                Ok(PrComment { id: row.get(0)?, author: row.get(1)?, body: row.get(2)?, created_at: row.get(3)?, path: row.get(4)?, line: row.get(5)? })
            })
            .optional()
            .map_err(|e| e.to_string())?
        {
            out.push(c);
        }
    }
    Ok(out)
}

fn mark_comments_resolved(db: &DbConnection, ids: &[String], resolution: &str) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().to_rfc3339();
    for id in ids {
        conn.execute("UPDATE pr_review_comments SET resolution = ?2, resolved_at = ?3 WHERE id = ?1", params![id, resolution, now]).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn fetch_pull_request_for_comment(db: &DbConnection, comment_id: &str) -> Result<Option<PullRequestInfo>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        &format!(
            "SELECT {} FROM pull_requests pr JOIN pr_review_comments c ON c.pull_request_id = pr.id WHERE c.id = ?1",
            pr_columns_from("pr")
        ),
        params![comment_id],
        row_to_pull_request,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// What a worktree session gets when the PR has no session of its own to
/// inherit from — a PR somebody else opened, or one discovered on a project
/// that has never run a session here.
const DEFAULT_PR_SESSION_EFFORT: &str = "high";
const DEFAULT_PR_SESSION_MODE: &str = "ask_edits";

/// Finds (or creates) the session whose worktree is genuinely checked out on
/// this PR's branch — what both "send the comments to a terminal" and "review
/// this PR" need. Same shape Devin/Cursor/Copilot's coding agents use: a
/// dedicated worktree on the PR's own branch, so the agent has the real diff
/// and can push straight back to update it.
///
/// 1. If the PR's own session is already on this exact branch (true for any
///    PR Hangar itself opened — it created that branch), reuse it, free.
/// 2. Else, if some other session in the project is already on this branch
///    (e.g. a previous review of this same PR already made one), reuse that.
/// 3. Else, create a real subsession: fetch the branch for real and add a
///    worktree on it, exactly like `create_subsession` does for a brand new
///    branch — just checking out one that already exists instead.
///
/// The project comes from the PR itself, not from a session: a PR opened by
/// someone else has no session at all, which is exactly the case steps 2 and 3
/// exist to serve. `account_id` is the account the new worktree session is
/// recorded against — the one the user picked, since for a stranger's PR there
/// is no previous choice to inherit.
fn resolve_session_for_pr_branch<R: Runtime>(app: &AppHandle<R>, db: &DbConnection, pr: &PullRequestInfo, account_id: &str) -> Result<String, String> {
    let head_branch = match &pr.head_branch {
        Some(b) if !b.is_empty() => b.clone(),
        _ => {
            let repo = format!("{}/{}", pr.repo_owner, pr.repo_name);
            let output = gh_output(None, &["pr", "view", &pr.number.to_string(), "--repo", &repo, "--json", "headRefName"])?;
            if !output.status.success() {
                return Err(format!("gh pr view failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
            }
            let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
            let branch = json
                .get("headRefName")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "gh did not return a headRefName".to_string())?
                .to_string();
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute("UPDATE pull_requests SET head_branch = ?2 WHERE id = ?1", params![pr.id, branch]).map_err(|e| e.to_string())?;
            branch
        }
    };

    let project_id = pr.project_id.clone();
    let project_path: String = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
            .map_err(|e| format!("the PR's project lookup failed: {e}"))?
    };

    // The PR's own session, when it has one. Gives two things: the branch it's
    // already on (step 1), and the model/effort/mode a worktree cut for this PR
    // should inherit. Absent for anything discovered, which is where the
    // defaults come in — a session row needs values for columns the terminal
    // itself never reads, since the CLI owns model and mode once it's running.
    let anchor: Option<(String, String, String, Option<String>)> = match &pr.session_id {
        Some(session_id) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT model, effort, mode, worktree_branch FROM sessions WHERE id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()
            .map_err(|e| e.to_string())?
        }
        None => None,
    };
    let anchor_worktree_branch = anchor.as_ref().and_then(|(_, _, _, branch)| branch.clone());
    let (session_model, session_effort, session_mode) = match &anchor {
        Some((model, effort, mode, _)) => (model.clone(), effort.clone(), mode.clone()),
        None => (String::new(), DEFAULT_PR_SESSION_EFFORT.to_string(), DEFAULT_PR_SESSION_MODE.to_string()),
    };

    // A local branch by the same name as `head_branch` might already be
    // checked out *anywhere* touching this repo — the user's own primary
    // clone (very plausibly the exact case if they're actively working on
    // this PR outside Hangar), another Hangar worktree, anything. Git refuses
    // to check out the same local branch name in two places at once, so
    // this Hangar-owned fallback name (deterministic from the PR number, so
    // it's the same every time and safely re-discoverable in step 2) only
    // gets used when the real name turns out to be unsafe — see the
    // `local_branch_exists` check below, right before the worktree is
    // actually created.
    let local_review_branch = format!("hangar/pr-{}-review", pr.number);
    // What the pre-rename build would have called the same branch. Only ever
    // matched, never created: a review worktree set up before the rename must
    // still be found and reused instead of a second one being cut for the
    // same PR (which then fights it over the branch).
    let legacy_review_branch = format!("fleet/pr-{}-review", pr.number);

    if let Some(session_id) = &pr.session_id {
        if (anchor_worktree_branch.as_deref() == Some(head_branch.as_str())
            || anchor_worktree_branch.as_deref() == Some(local_review_branch.as_str())
            || anchor_worktree_branch.as_deref() == Some(legacy_review_branch.as_str()))
            && session_worktree_is_on_disk(db, session_id)
        {
            return Ok(session_id.clone());
        }
    }

    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        // A previous "Send to agent" for this same PR could have landed on
        // any of the names, depending on whether there was a conflict at the
        // time and whether it ran before or after the rename — check all.
        let mut stmt = conn
            .prepare(
                "SELECT id, worktree_path FROM sessions WHERE project_id = ?1 AND worktree_branch IN (?2, ?3, ?4) ORDER BY started_at DESC",
            )
            .map_err(|e| e.to_string())?;
        let candidates: Vec<(String, Option<String>)> = stmt
            .query_map(params![project_id, head_branch, local_review_branch, legacy_review_branch], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        drop(stmt);
        drop(conn);

        // A row can outlive its directory — the user deletes the worktree by
        // hand, or from the Git panel, and the session stays behind. Reusing it
        // hands back a session whose cwd doesn't exist, which is what produced
        // "…/worktrees/pr-103 no longer exists on disk" at the moment the
        // terminal tried to open.
        // Same rule as the anchor check above, deliberately: a session with no
        // worktree runs in the project root, which exists by definition; one
        // with a path has to still have it.
        if let Some((existing, _)) = candidates
            .into_iter()
            .find(|(_, path)| path.as_deref().is_none_or(|p| Path::new(p).is_dir()))
        {
            return Ok(existing);
        }
    }

    // Git keeps its own registry of worktrees, and a directory deleted outside
    // git stays in it — so `worktree add` on that same path fails with "already
    // registered" until it's pruned. Pruning here is what makes recovering from
    // a hand-deleted worktree work at all.
    let _ = git_capture(Path::new(&project_path), &["worktree", "prune"]);

    let worktree_dir = unique_worktree_dir(&project_path, &format!("pr-{}", pr.number));
    let project_dir = Path::new(&project_path);
    // Same `.hangar/worktrees/...` location `create_subsession` uses — needs
    // the same gitignore safeguard so it doesn't show up as untracked
    // clutter (or worse, get accidentally staged) in the project's own
    // `git status` the first time this runs.
    ensure_worktree_dir_ignored(&project_path);

    // Fetching into a *remote-tracking* ref (no local branch name involved) is
    // never blocked by anything being checked out — only creating or moving a
    // local branch ref is.
    git_capture(project_dir, &["fetch", "origin", &head_branch])?;
    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    // Three cases, and getting the first two confused is what put review
    // worktrees on `hangar/pr-N-review` when they could have been on the PR's
    // own branch — so commits landed somewhere GitHub Desktop reported as a
    // different branch than the PR.
    //
    // A branch merely *existing* locally is no obstacle: `git worktree add
    // <dir> <branch>` checks it out happily. What git refuses is the same
    // branch checked out in two places at once, which is a much narrower
    // condition and the only one worth inventing a name for.
    let checked_out: Vec<String> = git_capture(project_dir, &["worktree", "list", "--porcelain"])
        .ok()
        .map(|out| crate::commands::sessions::parse_worktree_list(&out).into_iter().map(|(_, branch)| branch).collect())
        .unwrap_or_default();
    let checked_out_elsewhere = checked_out.iter().any(|branch| branch == &head_branch);

    // The fallback name outlives the worktree it was cut for: delete the
    // directory and `hangar/pr-103-review` is still a branch, so the next
    // attempt used to die on "a branch named … already exists". Existing is
    // fine — we attach to it below. Only *checked out somewhere else* forces a
    // new name, and it's the same rule as for the PR's own branch.
    let mut chosen_branch = if checked_out_elsewhere { local_review_branch.clone() } else { head_branch.clone() };
    let mut suffix = 2;
    while checked_out.iter().any(|branch| branch == &chosen_branch) {
        chosen_branch = format!("{local_review_branch}-{suffix}");
        suffix += 1;
    }
    let dir = worktree_dir.to_string_lossy().to_string();

    if local_branch_exists(project_dir, &chosen_branch) {
        // The branch is right there and free — attach to it. Its upstream is
        // whatever it already had, deliberately: for the PR's own branch that's
        // the user's setting, and a review branch we created already points at
        // the PR.
        git_capture(project_dir, &["worktree", "add", &dir, &chosen_branch])?;
    } else {
        // Doesn't exist here yet: cut it from the remote.
        git_capture(project_dir, &["worktree", "add", "-b", &chosen_branch, &dir, &format!("origin/{head_branch}")])?;
    }

    if chosen_branch != head_branch {
        // Track the real remote branch explicitly rather than relying on what
        // `-b … origin/<head>` inferred, so a plain `git push` from inside this
        // worktree updates the actual PR — which is the whole point when the
        // branch had to be renamed.
        git_capture(&worktree_dir, &["branch", "--set-upstream-to", &format!("origin/{head_branch}"), &chosen_branch])?;
        // A reused review branch can be behind the PR by however many commits
        // landed since. Fast-forward only: if it has commits of its own the
        // merge refuses and we leave it exactly as it was, because losing
        // someone's review commits is far worse than opening on a stale tree.
        let _ = git_capture(&worktree_dir, &["merge", "--ff-only", &format!("origin/{head_branch}")]);
    }

    let new_session_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, parent_session_id, worktree_path, worktree_branch, worktree_kind, account_id, model, effort, mode, status, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'linked', ?7, ?8, ?9, ?10, 'waiting', ?11)",
            params![
                new_session_id,
                format!("review #{}", pr.number),
                project_id,
                pr.session_id,
                worktree_dir.to_string_lossy().to_string(),
                chosen_branch,
                account_id,
                session_model,
                session_effort,
                session_mode,
                now,
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    crate::process_manager::emit_block(
        app,
        &new_session_id,
        "agent",
        &format!("↳ subsession created to work on PR #{} — linked worktree tracking `{head_branch}`", pr.number),
    );

    Ok(new_session_id)
}

/// Whether a session's worktree is still on disk.
///
/// The row and the directory are separate facts: deleting a worktree by hand,
/// or from the Git panel, leaves the session behind. Anything that reuses a
/// session has to ask, or it returns one whose working directory doesn't exist.
fn session_worktree_is_on_disk(db: &DbConnection, session_id: &str) -> bool {
    let Ok(conn) = db.0.lock() else { return false };
    let path: Option<String> = conn
        .query_row("SELECT worktree_path FROM sessions WHERE id = ?1", params![session_id], |row| row.get(0))
        .optional()
        .ok()
        .flatten()
        .flatten();
    match path {
        Some(path) => Path::new(&path).is_dir(),
        // No worktree at all means the session runs in the project root, which
        // exists by definition.
        None => true,
    }
}

/// The account a PR's work should run under: the one the user picked, else the
/// one its own session already uses, else the project's most recent session's.
///
/// The chain matters because the two ends are different situations. On your own
/// PR there is always something to inherit and asking would be noise; on a PR
/// somebody else opened there is nothing at all, so the choice has to come from
/// the caller — which is why the review button carries an account picker.
fn account_for_pr(db: &DbConnection, pr: &PullRequestInfo, chosen: Option<String>) -> Result<String, String> {
    if let Some(account_id) = chosen {
        return Ok(account_id);
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if let Some(session_id) = &pr.session_id {
        if let Some(account_id) = conn
            .query_row("SELECT account_id FROM sessions WHERE id = ?1", params![session_id], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?
        {
            return Ok(account_id);
        }
    }
    conn.query_row(
        "SELECT account_id FROM sessions WHERE project_id = ?1 ORDER BY started_at DESC LIMIT 1",
        params![pr.project_id],
        |row| row.get::<_, String>(0),
    )
    .optional()
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "no account to run this PR's worktree under — pick one, or start a session in this project first".to_string())
}

/// The one place new comments actually reach an agent — always an explicit
/// user action (the "Send to agent" button, from either the AgentPanel
/// banner or the PR review inbox screen), never triggered by the poller or
/// `check_pr_comments` on their own. Looks the comments up by id rather than
/// trusting client-supplied bodies, and marks them resolved so they drop out
/// of the inbox on both surfaces. Resolves the *real* target session itself
/// (see `resolve_session_for_pr_branch`) instead of trusting a session id
/// the frontend supplies — returns whichever session actually got the
/// message, so the caller can navigate there.
#[tauri::command]
pub async fn send_pr_comments_to_session(app: AppHandle, comment_ids: Vec<String>) -> Result<String, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let comments = fetch_comments_by_ids(&db, &comment_ids)?;
        if comments.is_empty() {
            return Err("no matching comments to send".to_string());
        }
        let pr = fetch_pull_request_for_comment(&db, &comment_ids[0])?.ok_or_else(|| "comment has no linked PR".to_string())?;
        let account_id = account_for_pr(&db, &pr, None)?;
        let target_session_id = resolve_session_for_pr_branch(&app, &db, &pr, &account_id)?;

        let prompt = format_pr_comments_prompt(&comments);
        crate::commands::sessions::send_message(app.clone(), db.clone(), target_session_id.clone(), prompt)?;
        mark_comments_resolved(&db, &comment_ids, "sent")?;
        Ok(target_session_id)
    })
    .await?
}

/// Collapses the multi-line comments prompt to a single line, so it can ride
/// on the `claude '<prompt>'` command line without tripping zsh's PS2
/// continuation prompt inside the quotes. Bullets become " · " separators;
/// the text stays perfectly readable in the terminal and in scrollback.
fn to_single_line(prompt: &str) -> String {
    prompt.split('\n').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// Same as `send_pr_comments_to_session`, but the review feedback lands in a
/// real **interactive** terminal instead of a headless turn: a worktree on the
/// PR's own branch (via `resolve_session_for_pr_branch`, unchanged) running
/// `claude '<the comments>'` so Claude Code opens already working on them. This
/// is the terminal-first replacement for "Send to agent". Returns the opened
/// terminal. `account_id` is the account the user picked; `None` keeps the old
/// behavior of inheriting whatever the PR's own session runs under.
#[tauri::command]
pub async fn send_pr_comments_to_terminal(
    app: AppHandle,
    comment_ids: Vec<String>,
    account_id: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<crate::terminals::TerminalInfo, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let comments = fetch_comments_by_ids(&db, &comment_ids)?;
        if comments.is_empty() {
            return Err("no matching comments to send".to_string());
        }
        let pr = fetch_pull_request_for_comment(&db, &comment_ids[0])?.ok_or_else(|| "comment has no linked PR".to_string())?;

        // Reuses the exact worktree-on-the-PR-branch resolution the session
        // path uses — the container session carries the worktree path, and the
        // terminal then runs under the chosen account.
        let account_id = account_for_pr(&db, &pr, account_id)?;
        let target_session_id = resolve_session_for_pr_branch(&app, &db, &pr, &account_id)?;

        let prompt = to_single_line(&format_pr_comments_prompt(&comments));

        // Already working there: type into it instead of opening a second
        // terminal on the same worktree, where two agents would edit the same
        // files without knowing about each other.
        if let Some(existing) = crate::terminals::terminal_for_session(&app, &target_session_id) {
            crate::terminals::send_line_to_terminal(&app, &existing.id, &prompt)?;
            mark_comments_resolved(&db, &comment_ids, "sent")?;
            return Ok(existing);
        }
        let info = crate::terminals::open_terminal(
            app.clone(),
            crate::terminals::TerminalTarget { kind: crate::terminals::TargetKind::Session, id: target_session_id },
            Some(account_id),
            Some(crate::terminals::InitCommand::Start),
            None, // let Hangar mint the session id
            Some(prompt),
            cols,
            rows,
        )?;
        mark_comments_resolved(&db, &comment_ids, "sent")?;
        Ok(info)
    })
    .await?
}

/// The prompt Claude Code opens on when you send a PR to be reviewed.
///
/// Pure so the wording is testable, and deliberately explicit about **not
/// publishing**: the worktree has a real remote and `gh` is authenticated, so
/// an agent that decides to be helpful could comment on a stranger's PR in your
/// name. Posting a review is a message to another person — it's the user's to
/// send, from the terminal, once they've read what the agent found.
fn format_pr_review_prompt(pr: &PullRequestInfo) -> String {
    let title = pr.title.clone().unwrap_or_else(|| format!("PR #{}", pr.number));
    let author = pr.author_login.clone().unwrap_or_else(|| "someone else".to_string());
    let base = pr.base_branch.clone().unwrap_or_else(|| "the base branch".to_string());
    format!(
        "Review pull request #{} of {}/{} — \"{title}\", opened by {author}. \
         This worktree is checked out on the PR's branch; read the diff against `origin/{base}` and review it: \
         correctness bugs first, then anything risky or unclear. \
         Do not push, comment, or post a review — just tell me what you find.",
        pr.number, pr.repo_owner, pr.repo_name
    )
}

/// Opens an interactive terminal to review a PR — the point of showing other
/// people's PRs at all. Cuts (or reuses) a worktree on the PR's branch exactly
/// like the comments path, under the account the user picked, with Claude Code
/// already reading the diff.
///
/// Takes a PR id rather than a session id: a PR someone else opened has no
/// session, which is precisely the case this exists for.
#[tauri::command]
pub async fn open_pr_review_terminal(app: AppHandle, pr_id: String, account_id: String, cols: u16, rows: u16) -> Result<crate::terminals::TerminalInfo, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let pr = fetch_pull_request_by_id(&db, &pr_id)?.ok_or_else(|| format!("no tracked PR with id {pr_id}"))?;
        let account_id = account_for_pr(&db, &pr, Some(account_id))?;
        let target_session_id = resolve_session_for_pr_branch(&app, &db, &pr, &account_id)?;

        let prompt = to_single_line(&format_pr_review_prompt(&pr));
        if let Some(existing) = crate::terminals::terminal_for_session(&app, &target_session_id) {
            crate::terminals::send_line_to_terminal(&app, &existing.id, &prompt)?;
            return Ok(existing);
        }

        crate::terminals::open_terminal(
            app.clone(),
            crate::terminals::TerminalTarget { kind: crate::terminals::TargetKind::Session, id: target_session_id },
            Some(account_id),
            Some(crate::terminals::InitCommand::Start),
            None, // let Hangar mint the session id
            Some(prompt),
            cols,
            rows,
        )
    })
    .await?
}

/// Clears comments out of the inbox without sending anything to an agent —
/// for feedback that doesn't need action (a plain "LGTM").
#[tauri::command]
pub fn dismiss_pr_comments(db: State<DbConnection>, comment_ids: Vec<String>) -> Result<(), String> {
    mark_comments_resolved(&db, &comment_ids, "dismissed")
}

/// Every open PR Hangar is tracking, grouped by project, each with its still-
/// unresolved review comments (if any) — the PR review inbox screen's one
/// data source. Reads straight from the local database; it does NOT itself
/// discover new PRs or re-check GitHub (`check_all_open_prs_now`/the
/// background poller do that) — call one of those first if a PR you know
/// exists on GitHub hasn't shown up yet. Shows every open PR (not just ones
/// with pending comments) so a project's PRs are visible at a glance; the
/// pending-comment count is what actually needs attention.
#[tauri::command]
pub fn list_pr_review_inbox(db: State<DbConnection>) -> Result<Vec<crate::models::ProjectPrInbox>, String> {
    list_pr_review_inbox_impl(&db)
}

fn list_pr_review_inbox_impl(db: &DbConnection) -> Result<Vec<crate::models::ProjectPrInbox>, String> {
    use crate::models::{PrInboxEntry, ProjectPrInbox};

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // The project comes from the PR's own `project_id`, and the session is a
    // LEFT JOIN: a PR opened by someone else has no session, and joining
    // through one is what used to make it unshowable.
    let mut stmt = conn
        .prepare(&format!(
            "SELECT p.id, p.name, s.id, s.name,
                    {},
                    c.id, c.author, c.body, c.comment_created_at, c.path, c.line
             FROM pull_requests pr
             JOIN projects p ON p.id = pr.project_id
             LEFT JOIN sessions s ON s.id = pr.session_id
             LEFT JOIN pr_review_comments c ON c.pull_request_id = pr.id AND c.resolved_at IS NULL
             WHERE pr.state = 'open'
             ORDER BY p.name, pr.relation, pr.created_at, c.comment_created_at",
            pr_columns_from("pr")
        ))
        .map_err(|e| e.to_string())?;

    struct Row {
        project_id: String,
        project_name: String,
        session_id: Option<String>,
        session_name: Option<String>,
        pull_request: PullRequestInfo,
        comment: Option<PrComment>,
    }
    // `PR_COLUMNS` starts at index 4 here (project id/name and session id/name
    // come first), and the comment columns follow it.
    const PR_AT: usize = 4;
    let comment_at = PR_AT + PR_COLUMNS.split(", ").count();
    let rows: Vec<Row> = stmt
        .query_map([], |row| {
            let comment_id: Option<String> = row.get(comment_at)?;
            Ok(Row {
                project_id: row.get(0)?,
                project_name: row.get(1)?,
                session_id: row.get(2)?,
                session_name: row.get(3)?,
                pull_request: PullRequestInfo {
                    id: row.get(PR_AT)?,
                    project_id: row.get(PR_AT + 1)?,
                    session_id: row.get(PR_AT + 2)?,
                    repo_owner: row.get(PR_AT + 3)?,
                    repo_name: row.get(PR_AT + 4)?,
                    number: row.get(PR_AT + 5)?,
                    url: row.get(PR_AT + 6)?,
                    state: row.get(PR_AT + 7)?,
                    last_comment_seen_at: row.get(PR_AT + 8)?,
                    created_at: row.get(PR_AT + 9)?,
                    last_check_error: row.get(PR_AT + 10)?,
                    // Purely an internal freshness cache for check_pr_comments_for's
                    // fast-path decision — not needed for display, and skipped
                    // from serialization anyway.
                    last_seen_updated_at: None,
                    head_branch: row.get(PR_AT + 12)?,
                    base_branch: row.get(PR_AT + 13)?,
                    title: row.get(PR_AT + 14)?,
                    author_login: row.get(PR_AT + 15)?,
                    relation: row.get(PR_AT + 16)?,
                },
                comment: match comment_id {
                    Some(id) => Some(PrComment {
                        id,
                        author: row.get(comment_at + 1)?,
                        body: row.get(comment_at + 2)?,
                        created_at: row.get(comment_at + 3)?,
                        path: row.get(comment_at + 4)?,
                        line: row.get(comment_at + 5)?,
                    }),
                    None => None,
                },
            })
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    let mut projects: Vec<ProjectPrInbox> = Vec::new();
    for row in rows {
        let project_idx = match projects.iter().position(|p| p.project_id == row.project_id) {
            Some(i) => i,
            None => {
                projects.push(ProjectPrInbox { project_id: row.project_id, project_name: row.project_name, entries: Vec::new() });
                projects.len() - 1
            }
        };
        let project = &mut projects[project_idx];
        let entry_idx = match project.entries.iter().position(|e| e.pull_request.id == row.pull_request.id) {
            Some(i) => i,
            None => {
                project.entries.push(PrInboxEntry {
                    pull_request: row.pull_request,
                    session_id: row.session_id,
                    session_name: row.session_name,
                    comments: Vec::new(),
                });
                project.entries.len() - 1
            }
        };
        if let Some(comment) = row.comment {
            project.entries[entry_idx].comments.push(comment);
        }
    }
    Ok(projects)
}

/// Manual "check now" for the PR review inbox screen — runs the same real
/// `gh` discovery + comment check the background poller does, immediately:
/// finds any open PR of yours on a known project's repo that Hangar isn't
/// tracking yet, then checks every tracked open PR for new comments (not
/// just the one session `check_pr_comments` targets). The caller re-fetches
/// `list_pr_review_inbox` afterward to see the result.
#[tauri::command]
pub async fn check_all_open_prs_now(app: AppHandle) -> Result<(), String> {
    crate::blocking::run_blocking(move || {
        poll_all_open_prs(&app);
    })
    .await
}

/// Records every open PR on one project's repo — yours, the ones waiting on
/// your review, and the rest — from a snapshot already fetched for that repo.
///
/// Takes the snapshot rather than fetching: the same round trip also feeds the
/// comment check, so one query per repo serves the whole pass. A discovered PR
/// gets no session (only a PR Hangar opened itself carries the session that
/// produced it), and errors are the caller's to log, not surface, since this
/// runs opportunistically alongside a real check.
fn record_open_prs_for_project(db: &DbConnection, project_id: &str, owner: &str, repo: &str, snapshot: &RepoPrSnapshot) -> Result<(), String> {
    if snapshot.prs.is_empty() {
        return Ok(());
    }
    let self_login = snapshot.viewer_login.as_deref();
    let still_open: Vec<i64> = snapshot.prs.iter().map(|pr| pr.number).collect();

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    prune_closed_foreign_prs(&conn, owner, repo, &still_open)?;
    for pr in &snapshot.prs {
        let assigned_to_me = pr.assignees.iter().any(|a| Some(a.as_str()) == self_login);
        let relation = classify_pr(pr.author_login.as_deref(), self_login, snapshot.review_requested.contains(&pr.number), assigned_to_me);

        // Everything except `relation` is written once at discovery, but the
        // relation is re-evaluated every pass: being asked to review a PR that
        // was nothing to you yesterday is the normal course of a week. Title
        // and branches refresh too — a PR gets retitled and retargeted.
        let updated = conn
            .execute(
                "UPDATE pull_requests SET relation = ?4, author_login = ?5, title = ?6, head_branch = COALESCE(?7, head_branch), base_branch = COALESCE(?8, base_branch)
                 WHERE repo_owner = ?1 AND repo_name = ?2 AND number = ?3",
                params![owner, repo, pr.number, relation, pr.author_login, pr.title, pr.head_branch, pr.base_branch],
            )
            .map_err(|e| e.to_string())?;
        if updated > 0 {
            continue;
        }

        conn.execute(
            "INSERT INTO pull_requests (id, project_id, session_id, repo_owner, repo_name, number, url, state, created_at, head_branch, base_branch, title, author_login, relation)
             VALUES (?1,?2,NULL,?3,?4,?5,?6,'open',?7,?8,?9,?10,?11,?12)",
            params![
                Uuid::new_v4().to_string(),
                project_id,
                owner,
                repo,
                pr.number,
                pr.url,
                pr.created_at,
                pr.head_branch,
                pr.base_branch,
                pr.title,
                pr.author_login,
                relation
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Drops tracked PRs that aren't yours and are no longer open on GitHub.
///
/// Your own PRs are checked every pass and cleaned up when they merge, but
/// nothing polls the other two tabs (see `open_prs_to_check`) — without this a
/// stranger's PR would sit in the list forever after being merged. Discovery's
/// own answer is the authority: `gh pr list --state open` *is* the set of open
/// PRs, so anything else tracked for this repo is closed. No extra `gh` call.
///
/// Skipped when the list came back full: at the limit there may be open PRs
/// that simply didn't fit, and deleting those would churn them in and out on
/// every pass.
fn prune_closed_foreign_prs(conn: &rusqlite::Connection, owner: &str, repo: &str, still_open: &[i64]) -> Result<(), String> {
    if still_open.len() >= DISCOVERY_PR_LIMIT {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", still_open.len()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "DELETE FROM pull_requests WHERE repo_owner = ?1 AND repo_name = ?2 AND relation != 'mine' AND number NOT IN ({placeholders})"
    );
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(owner.to_string()), Box::new(repo.to_string())];
    args.extend(still_open.iter().map(|n| Box::new(*n) as Box<dyn rusqlite::ToSql>));
    conn.execute(&sql, rusqlite::params_from_iter(args.iter().map(|a| a.as_ref()))).map_err(|e| e.to_string())?;
    Ok(())
}

/// How many open PRs a single repo contributes. A busy repo's full list isn't
/// a work queue, and every extra row is one more card on a screen whose point
/// is deciding what to look at next. `gh`'s own default is 30.
const DISCOVERY_PR_LIMIT: usize = 50;

/// What an open PR is to the logged-in user. Pure, because this is the rule the
/// whole three-tab split rests on and it should be readable without a `gh` call.
///
/// With no known login (a `gh api user` that failed), everything falls to
/// `other` except what GitHub itself flagged as review-requested: guessing
/// "mine" without knowing who I am would put strangers' PRs in the tab whose
/// comments get polled and badged.
fn classify_pr(author: Option<&str>, self_login: Option<&str>, review_requested: bool, assigned_to_me: bool) -> &'static str {
    if self_login.is_some() && author.is_some() && author == self_login {
        "mine"
    } else if review_requested || assigned_to_me {
        "review"
    } else {
        "other"
    }
}

/// Every tracked open PR that belongs to one repo, with its own row id — what
/// lets a repo's single snapshot be matched against what Hangar already knows.
///
/// **Only `mine`**, and that's the line that keeps this affordable. On someone
/// else's PR the comments are other people reviewing *their* code, not a queue
/// of yours; and since discovery brings in every open PR of every project's
/// repo, checking them all would mean parsing and writing dozens of PRs' comment
/// histories every five minutes to badge things nobody asked to be notified
/// about.
fn tracked_open_prs_for_repo(conn: &rusqlite::Connection, owner: &str, repo: &str) -> Vec<(String, i64)> {
    let Ok(mut stmt) =
        conn.prepare("SELECT id, number FROM pull_requests WHERE repo_owner = ?1 AND repo_name = ?2 AND state = 'open' AND relation = 'mine'")
    else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(params![owner, repo], |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))) else {
        return Vec::new();
    };
    rows.filter_map(Result::ok).collect()
}

/// Background poller (see `lib.rs`'s `.setup()`) — **one GraphQL round trip per
/// repo** that does discovery and the comment check together, then emits an
/// event for any PR with new comments and fires the opt-in notification. Never
/// sends anything to an agent by itself.
///
/// This is the shape the whole pass was rebuilt around. It used to cost two `gh`
/// calls per repo plus one to three per tracked PR, each a process spawn:
/// measured on this machine, 11-21 processes and 10-20 seconds for three
/// projects. Now it's one 0.4-0.8s call per repo.
pub fn poll_all_open_prs<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DbConnection>() else { return };
    let projects: Vec<(String, String)> = {
        let Ok(conn) = db.0.lock() else { return };
        let Ok(mut stmt) = conn.prepare("SELECT id, path FROM projects") else { return };
        let Ok(rows) = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) else { return };
        rows.filter_map(Result::ok).collect()
    };

    for (project_id, project_path) in projects {
        let Ok((owner, repo)) = detect_repo_owner_and_name(Path::new(&project_path)) else {
            continue; // no git remote — nothing to discover, and not an error
        };
        let snapshot = match fetch_repo_pr_snapshot(&owner, &repo, DISCOVERY_PR_LIMIT) {
            Ok(snapshot) => snapshot,
            Err(e) => {
                log::error!("poll_all_open_prs: snapshot failed for {owner}/{repo}: {e}");
                continue;
            }
        };
        if let Err(e) = record_open_prs_for_project(&db, &project_id, &owner, &repo, &snapshot) {
            log::error!("poll_all_open_prs: recording {owner}/{repo}'s PRs failed: {e}");
        }

        let tracked = {
            let Ok(conn) = db.0.lock() else { return };
            tracked_open_prs_for_repo(&conn, &owner, &repo)
        };
        for (pr_id, number) in tracked {
            let Ok(Some(pr)) = fetch_pull_request_by_id(&db, &pr_id) else { continue };
            let session_id = pr.session_id.clone();
            // A tracked PR missing from a snapshot of *open* PRs has been merged
            // or closed, and the snapshot can't say which — that one gets its
            // own query, which is rare and is what resolves it exactly.
            let facts = match snapshot.prs.iter().find(|p| p.number == number) {
                Some(found) => Ok(PrFacts {
                    state: found.state.clone(),
                    updated_at: found.updated_at.clone(),
                    comments: found.comments.clone(),
                    viewer_login: snapshot.viewer_login.clone(),
                }),
                None => fetch_single_pr_facts(&pr),
            };
            match check_pr_comments_with(&db, pr, facts) {
                Ok(comments) if !comments.is_empty() => {
                    let _ = app.emit("pr-comments-found", serde_json::json!({ "sessionId": session_id, "comments": comments }));
                    crate::notifications::notify_if_enabled(
                        app,
                        |s| s.notify_pr_comments,
                        "New PR comments",
                        &format!("{} new comment(s) on a PR Hangar opened", comments.len()),
                    );
                }
                Ok(_) => {}
                Err(e) => log::error!("poll_all_open_prs: check failed for PR {pr_id}: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_owner_repo_from_url_handles_https_and_ssh_forms() {
        assert_eq!(parse_owner_repo_from_url("https://github.com/killua15/Hangar.git"), Some(("killua15".to_string(), "Hangar".to_string())));
        assert_eq!(parse_owner_repo_from_url("https://github.com/killua15/Hangar"), Some(("killua15".to_string(), "Hangar".to_string())));
        assert_eq!(parse_owner_repo_from_url("git@github.com:killua15/Hangar.git"), Some(("killua15".to_string(), "Hangar".to_string())));
    }

    #[test]
    fn parse_owner_repo_from_url_rejects_non_github_urls() {
        assert_eq!(parse_owner_repo_from_url("https://gitlab.com/killua15/Hangar.git"), None);
        assert_eq!(parse_owner_repo_from_url("not a url"), None);
    }

    #[test]
    fn group_name_status_buckets_by_a_m_d() {
        let (added, modified, removed) = group_name_status("A\tnew.txt\nM\tchanged.txt\nD\tgone.txt\nR100\told.txt\tnew_name.txt");
        assert_eq!(added, vec!["new.txt"]);
        assert_eq!(modified, vec!["changed.txt", "new_name.txt"], "renames fold into modified, keyed by the new path");
        assert_eq!(removed, vec!["gone.txt"]);
    }

    #[test]
    fn render_pr_body_skips_empty_groups_and_always_has_footer() {
        let body = render_pr_body(&["new.txt".to_string()], &[], &[]);
        assert!(body.contains("### Added"));
        assert!(!body.contains("### Modified"));
        assert!(!body.contains("### Removed"));
        assert!(body.contains("_Opened by Hangar._"));
    }

    #[test]
    fn format_pr_comments_prompt_lists_every_comment() {
        let comments = vec![
            PrComment {
                id: "c1".to_string(),
                author: "alice".to_string(),
                body: "please fix the typo".to_string(),
                created_at: "2026-01-01T00:00:00Z".to_string(),
                path: None,
                line: None,
            },
            PrComment {
                id: "c2".to_string(),
                author: "bob".to_string(),
                body: "looks good otherwise".to_string(),
                created_at: "2026-01-01T00:01:00Z".to_string(),
                path: None,
                line: None,
            },
        ];
        let prompt = format_pr_comments_prompt(&comments);
        assert!(prompt.contains("alice: please fix the typo"));
        assert!(prompt.contains("bob: looks good otherwise"));
        assert!(prompt.contains("Please address these."));
    }

    #[test]
    fn format_pr_comments_prompt_includes_file_and_line_for_inline_comments() {
        let comments = vec![PrComment {
            id: "c1".to_string(),
            author: "cmoura2".to_string(),
            body: "this lookup can return null".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            path: Some("BridgeRepository.cs".to_string()),
            line: Some(40),
        }];
        let prompt = format_pr_comments_prompt(&comments);
        assert!(prompt.contains("cmoura2 (BridgeRepository.cs:40): this lookup can return null"));
    }

    /// The terminal path rides the prompt on a `claude '<prompt>'` command
    /// line, so it must be one line — a newline inside the quotes would make
    /// zsh sit at its PS2 continuation prompt. Every comment must survive the
    /// collapse; only the line breaks change.
    #[test]
    fn to_single_line_flattens_the_prompt_but_keeps_every_comment() {
        let comments = vec![
            PrComment { id: "c1".into(), author: "alice".into(), body: "fix the typo".into(), created_at: "t".into(), path: None, line: None },
            PrComment { id: "c2".into(), author: "bob".into(), body: "rename this".into(), created_at: "t".into(), path: None, line: None },
        ];
        let line = to_single_line(&format_pr_comments_prompt(&comments));
        assert!(!line.contains('\n'), "the command-line prompt must be a single line: {line}");
        assert!(line.contains("alice: fix the typo"));
        assert!(line.contains("bob: rename this"));
        assert!(line.contains("Please address these."));
    }

    // -- the one-query-per-repo snapshot ------------------------------------

    /// A response in the exact shape the real query returns (verified against
    /// `cli/cli` before this was written): one PR with all three comment
    /// sources, an empty-bodied approval, and an inline comment whose `line`
    /// GitHub has nulled out.
    fn snapshot_json() -> Value {
        serde_json::from_str(
            r#"{
              "data": {
                "viewer": {"login": "killua15"},
                "repository": {"pullRequests": {"nodes": [
                  {
                    "number": 128, "url": "https://github.com/killua15/Hangar/pull/128",
                    "title": "Fix billing rounding", "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-02T00:00:00Z", "state": "OPEN",
                    "author": {"login": "alice"},
                    "assignees": {"nodes": [{"login": "killua15"}]},
                    "headRefName": "feat/billing", "baseRefName": "main",
                    "comments": {"nodes": [
                      {"id": "IC_kwDOaaa", "body": "please fix the typo", "createdAt": "2026-01-01T00:00:00Z", "author": {"login": "alice"}}
                    ]},
                    "reviews": {"nodes": [
                      {"id": "PRR_kwDObbb", "body": "", "submittedAt": "2026-01-01T00:01:00Z", "author": {"login": "bob"}},
                      {"id": "PRR_kwDOccc", "body": "one more thing", "submittedAt": "2026-01-01T00:02:00Z", "author": {"login": "carol"}}
                    ]},
                    "reviewThreads": {"nodes": [
                      {"comments": {"nodes": [
                        {"databaseId": 4772643945, "body": "this lookup can return null", "createdAt": "2026-01-01T00:03:00Z", "path": "src/billing.rs", "line": 40, "originalLine": 40, "author": {"login": "dave"}},
                        {"databaseId": 4772643946, "body": "and a null check here", "createdAt": "2026-01-01T00:04:00Z", "path": "src/billing.rs", "line": null, "originalLine": 62, "author": {"login": "dave"}}
                      ]}}
                    ]}
                  }
                ]}},
                "reviewRequested": {"nodes": [{"number": 128}]}
              }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn the_snapshot_carries_the_viewer_the_review_requests_and_the_prs() {
        let snapshot = parse_repo_snapshot(&snapshot_json());
        assert_eq!(snapshot.viewer_login.as_deref(), Some("killua15"), "the viewer comes free in the same query — no separate `gh api user`");
        assert!(snapshot.review_requested.contains(&128));
        assert_eq!(snapshot.prs.len(), 1);

        let pr = &snapshot.prs[0];
        assert_eq!(pr.number, 128);
        assert_eq!(pr.state, "open", "GitHub says OPEN; the column stores lowercase");
        assert_eq!(pr.author_login.as_deref(), Some("alice"));
        assert_eq!(pr.assignees, vec!["killua15".to_string()]);
        assert_eq!(pr.base_branch.as_deref(), Some("main"));
    }

    /// The three comment sources GitHub keeps apart arrive merged, oldest
    /// first, with bodyless reviews dropped — the same contract the two REST
    /// parsers used to provide between them.
    #[test]
    fn one_query_returns_all_three_comment_sources_merged_and_ordered() {
        let snapshot = parse_repo_snapshot(&snapshot_json());
        let comments = &snapshot.prs[0].comments;

        assert_eq!(comments.len(), 4, "1 comment + 1 review with a body + 2 thread comments; the empty approval is dropped");
        let authors: Vec<&str> = comments.iter().map(|c| c.author.as_str()).collect();
        assert_eq!(authors, vec!["alice", "carol", "dave", "dave"], "oldest first");

        assert_eq!(comments[2].path.as_deref(), Some("src/billing.rs"));
        assert_eq!(comments[2].line, Some(40));
        assert_eq!(comments[3].line, Some(62), "must fall back to originalLine once GitHub nulls out line");
    }

    /// **The migration hazard.** `external_id` is how dedup works, and rows
    /// already in the database were written from `gh pr view --json
    /// comments,reviews` (GraphQL node ids) and from the REST inline endpoint
    /// (numeric ids). If GraphQL's field choice produced different strings,
    /// every comment ever seen would look new and the inbox would flood on the
    /// first pass. Verified against real API responses: `gh pr view`'s
    /// `reviews[].id` is byte-identical to GraphQL's `id`.
    #[test]
    fn comment_ids_keep_the_shape_already_persisted_so_nothing_looks_new() {
        let snapshot = parse_repo_snapshot(&snapshot_json());
        let ids: Vec<&str> = snapshot.prs[0].comments.iter().map(|c| c.external_id.as_str()).collect();

        assert_eq!(ids[0], "IC_kwDOaaa", "issue comments keep the node id");
        assert_eq!(ids[1], "PRR_kwDOccc", "reviews keep the node id");
        assert_eq!(ids[2], "4772643945", "inline comments keep the REST numeric id, via databaseId");
        assert_eq!(ids[3], "4772643946");
    }

    /// A repo with nothing open must parse to an empty snapshot rather than
    /// failing — that's the steady state of most projects.
    #[test]
    fn an_empty_repo_parses_to_an_empty_snapshot() {
        let json: Value =
            serde_json::from_str(r#"{"data":{"viewer":{"login":"killua15"},"repository":{"pullRequests":{"nodes":[]}},"reviewRequested":{"nodes":[]}}}"#).unwrap();
        let snapshot = parse_repo_snapshot(&json);
        assert!(snapshot.prs.is_empty());
        assert!(snapshot.review_requested.is_empty());
        assert_eq!(snapshot.viewer_login.as_deref(), Some("killua15"));
    }

    fn raw(external_id: &str, author: &str, body: &str, created_at: &str) -> RawPrComment {
        RawPrComment { external_id: external_id.to_string(), author: author.to_string(), body: body.to_string(), created_at: created_at.to_string(), path: None, line: None }
    }

    #[test]
    fn select_new_comments_from_others_drops_self_authored_comments() {
        let all = vec![raw("c1", "me", "own follow-up", "2026-01-01T00:00:00Z"), raw("c2", "alice", "please fix the typo", "2026-01-01T00:01:00Z")];
        let selected = select_new_comments_from_others(all, &HashSet::new(), Some("me"));
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].author, "alice");
    }

    #[test]
    fn select_new_comments_from_others_drops_already_tracked_ids_regardless_of_timestamp() {
        // "old" here is chronologically earlier than "new" but was never
        // persisted before — the exact bug this dedup strategy fixes: a
        // comment source that used to be invisible (e.g. inline comments,
        // before that support existed) must still surface once it's finally
        // fetched, even though its timestamp predates comments Hangar has
        // already seen and marked tracked.
        let all = vec![raw("old", "alice", "old one, never tracked before", "2026-01-01T00:00:00Z"), raw("new", "bob", "new one, already tracked", "2026-01-02T00:00:00Z")];
        let already_tracked = HashSet::from(["new".to_string()]);
        let selected = select_new_comments_from_others(all, &already_tracked, None);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].external_id, "old", "the untracked comment must surface even though it's chronologically older");
    }

    #[test]
    fn select_new_comments_from_others_with_no_login_or_tracked_ids_keeps_everything() {
        let all = vec![raw("c1", "alice", "a", "2026-01-01T00:00:00Z"), raw("c2", "bob", "b", "2026-01-02T00:00:00Z")];
        let selected = select_new_comments_from_others(all, &HashSet::new(), None);
        assert_eq!(selected.len(), 2);
    }

    #[test]
    fn decide_check_outcome_merged_means_cleanup_regardless_of_anything_else() {
        assert_eq!(decide_check_outcome("merged", "2026-01-02T00:00:00Z", Some("2026-01-02T00:00:00Z")), CheckOutcome::Cleanup);
        assert_eq!(decide_check_outcome("merged", "2026-01-02T00:00:00Z", None), CheckOutcome::Cleanup);
    }

    #[test]
    fn decide_check_outcome_closed_without_merge_means_pause() {
        assert_eq!(decide_check_outcome("closed", "2026-01-02T00:00:00Z", None), CheckOutcome::Pause);
    }

    #[test]
    fn decide_check_outcome_open_with_unchanged_updated_at_skips_the_full_fetch() {
        assert_eq!(decide_check_outcome("open", "2026-01-02T00:00:00Z", Some("2026-01-02T00:00:00Z")), CheckOutcome::NothingChanged);
    }

    #[test]
    fn decide_check_outcome_open_with_changed_or_first_ever_updated_at_needs_a_full_fetch() {
        assert_eq!(
            decide_check_outcome("open", "2026-01-03T00:00:00Z", Some("2026-01-02T00:00:00Z")),
            CheckOutcome::NeedsFullFetch,
            "updated_at moved since the last check"
        );
        assert_eq!(decide_check_outcome("open", "2026-01-02T00:00:00Z", None), CheckOutcome::NeedsFullFetch, "never checked before");
    }

    /// In-memory sqlite with the real schema, plus one project → session →
    /// pull_request row, so `list_pr_review_inbox`/`mark_comments_resolved`/
    /// `fetch_comments_by_ids` can be exercised against real SQL instead of
    /// being trusted by inspection.
    fn setup_db_with_open_pr(project_id: &str, project_name: &str, pr_id: &str) -> DbConnection {
        use rusqlite::Connection;
        use std::sync::Mutex as StdMutex;

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();

        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES (?1,?2,'/tmp/proj','now')", params![project_id, project_name]).unwrap();
        let session_id = format!("session-{pr_id}");
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES (?1,'a session',?2,'acc1','sonnet','high','ask_edits','waiting','now')",
            params![session_id, project_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pull_requests (id, project_id, session_id, repo_owner, repo_name, number, url, state, relation, created_at)
             VALUES (?1,?2,?3,'killua15','Hangar',1,'https://github.com/killua15/Hangar/pull/1','open','mine','now')",
            params![pr_id, project_id, session_id],
        )
        .unwrap();

        DbConnection(StdMutex::new(conn))
    }

    /// Adds a second open PR (own session) to a project already seeded by
    /// `setup_db_with_open_pr`, for tests with more than one PR per project.
    fn insert_open_pr(db: &DbConnection, project_id: &str, pr_id: &str) {
        let conn = db.0.lock().unwrap();
        let session_id = format!("session-{pr_id}");
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES (?1,'another session',?2,'acc1','sonnet','high','ask_edits','waiting','now')",
            params![session_id, project_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO pull_requests (id, project_id, session_id, repo_owner, repo_name, number, url, state, relation, created_at)
             VALUES (?1,?2,?3,'killua15','Hangar',2,'https://github.com/killua15/Hangar/pull/2','open','mine','now')",
            params![pr_id, project_id, session_id],
        )
        .unwrap();
    }

    /// Adds another open PR sharing an existing session — the shape a project
    /// gets when its one session opens several PRs.
    fn insert_open_pr_sharing_session(db: &DbConnection, session_id: &str, pr_id: &str, number: i64, created_at: &str) {
        let conn = db.0.lock().unwrap();
        let project_id: String = conn.query_row("SELECT project_id FROM sessions WHERE id = ?1", params![session_id], |r| r.get(0)).unwrap();
        conn.execute(
            "INSERT INTO pull_requests (id, project_id, session_id, repo_owner, repo_name, number, url, state, relation, created_at)
             VALUES (?1,?2,?3,'killua15','Hangar',?4,?5,'open','mine',?6)",
            params![pr_id, project_id, session_id, number, format!("https://github.com/killua15/Hangar/pull/{number}"), created_at],
        )
        .unwrap();
    }

    /// A PR nobody here opened: no session, an author who isn't the user, and
    /// a relation that puts it in one of the two new tabs.
    fn insert_someone_elses_pr(db: &DbConnection, project_id: &str, pr_id: &str, number: i64, author: &str, relation: &str) {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO pull_requests (id, project_id, session_id, repo_owner, repo_name, number, url, state, title, author_login, relation, head_branch, base_branch, created_at)
             VALUES (?1,?2,NULL,'killua15','Hangar',?3,?4,'open','Their change',?5,?6,'feat/theirs','main','now')",
            params![pr_id, project_id, number, format!("https://github.com/killua15/Hangar/pull/{number}"), author, relation],
        )
        .unwrap();
    }

    /// The exact bug reported live: a project with three open PRs
    /// discovered outside Hangar, all sharing the project's one session_id.
    /// `fetch_pull_request` (session-keyed, `ORDER BY created_at DESC LIMIT
    /// 1`) can only ever reach the newest of them — `fetch_pull_request_by_id`
    /// (used by the poller/inbox "check now" path) must reach every one.
    #[test]
    fn fetch_pull_request_by_id_reaches_every_pr_even_when_they_share_a_session() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr-old");
        let shared_session_id = "session-pr-old";
        insert_open_pr_sharing_session(&db, shared_session_id, "pr-mid", 2, "2026-01-02T00:00:00Z");
        insert_open_pr_sharing_session(&db, shared_session_id, "pr-new", 3, "2026-01-03T00:00:00Z");

        // The session-keyed lookup can only ever surface one of the three —
        // never all of them, regardless of which one "wins" the ordering.
        let via_session = fetch_pull_request(&db, shared_session_id).unwrap().unwrap();
        assert!(["pr-old", "pr-mid", "pr-new"].contains(&via_session.id.as_str()));

        // But by-id lookup must reach every single one — this is what the
        // poller and "check now" actually iterate over.
        for (pr_id, expected_number) in [("pr-old", 1), ("pr-mid", 2), ("pr-new", 3)] {
            let pr = fetch_pull_request_by_id(&db, pr_id).unwrap().unwrap_or_else(|| panic!("{pr_id} must be reachable by its own id"));
            assert_eq!(pr.number, expected_number);
        }
    }

    fn insert_review_comment(db: &DbConnection, id: &str, pr_id: &str, author: &str, body: &str) {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO pr_review_comments (id, pull_request_id, author, body, comment_created_at, created_at) VALUES (?1,?2,?3,?4,'2026-01-01T00:00:00Z','2026-01-01T00:00:00Z')",
            params![id, pr_id, author, body],
        )
        .unwrap();
    }

    #[test]
    fn list_pr_review_inbox_groups_by_project_and_only_includes_pending_comments() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_review_comment(&db, "c1", "pr1", "alice", "please fix the typo");
        insert_review_comment(&db, "c2", "pr1", "bob", "one more thing");

        let inbox = list_pr_review_inbox_impl(&db).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].project_name, "Project One");
        assert_eq!(inbox[0].entries.len(), 1);
        assert_eq!(inbox[0].entries[0].comments.len(), 2);
    }

    /// Every open PR shows up in the inbox regardless of whether it has any
    /// pending comments — this is a "what's open" view, not just a
    /// comments-only alert list. A PR with only resolved comments still
    /// appears, just with zero comments attached.
    #[test]
    fn list_pr_review_inbox_still_lists_open_prs_with_no_pending_comments() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_review_comment(&db, "c1", "pr1", "alice", "already handled");
        mark_comments_resolved(&db, &["c1".to_string()], "dismissed").unwrap();

        let inbox = list_pr_review_inbox_impl(&db).unwrap();
        assert_eq!(inbox.len(), 1, "the open PR must still appear even with nothing pending");
        assert_eq!(inbox[0].entries.len(), 1);
        assert!(inbox[0].entries[0].comments.is_empty(), "the resolved comment must not count as pending");
    }

    /// The exact scenario reported live: a project with two open PRs, only
    /// one of which has a pending comment — both must still show up in the
    /// project's entry list.
    #[test]
    fn list_pr_review_inbox_shows_every_open_pr_in_a_project_even_without_comments() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_open_pr(&db, "proj1", "pr2");
        insert_review_comment(&db, "c1", "pr1", "alice", "please fix the typo");

        let inbox = list_pr_review_inbox_impl(&db).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].entries.len(), 2, "both open PRs must appear, not just the one with a pending comment");
        let with_comment = inbox[0].entries.iter().find(|e| e.pull_request.id == "pr1").unwrap();
        let without_comment = inbox[0].entries.iter().find(|e| e.pull_request.id == "pr2").unwrap();
        assert_eq!(with_comment.comments.len(), 1);
        assert!(without_comment.comments.is_empty());
    }

    /// A PR whose check is failing must still appear (with the error
    /// attached) even with zero comments — the whole point is that this
    /// state must be distinguishable from "checked fine, nothing new".
    #[test]
    fn list_pr_review_inbox_surfaces_a_pr_with_a_failing_check() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            conn.execute("UPDATE pull_requests SET last_check_error = 'gh pr view failed: some real reason' WHERE id = 'pr1'", []).unwrap();
        }

        let inbox = list_pr_review_inbox_impl(&db).unwrap();
        assert_eq!(inbox[0].entries.len(), 1);
        assert_eq!(inbox[0].entries[0].pull_request.last_check_error.as_deref(), Some("gh pr view failed: some real reason"));
        assert!(inbox[0].entries[0].comments.is_empty(), "no comments and a check error can coexist — that's the whole point");
    }

    #[test]
    fn dismiss_pr_comments_marks_resolution_and_clears_it_from_pending_comments() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_review_comment(&db, "c1", "pr1", "alice", "please fix the typo");

        mark_comments_resolved(&db, &["c1".to_string()], "dismissed").unwrap();

        let inbox = list_pr_review_inbox_impl(&db).unwrap();
        assert_eq!(inbox.len(), 1, "the PR itself stays in the inbox");
        assert!(inbox[0].entries[0].comments.is_empty(), "the dismissed comment must no longer count as pending");
    }

    /// Confirms the actual mechanism `CheckOutcome::Cleanup` relies on:
    /// deleting a `pull_requests` row must cascade-delete its
    /// `pr_review_comments` too (the schema's `ON DELETE CASCADE`), so a
    /// merged PR's comments don't linger as orphaned rows nobody can reach.
    #[test]
    fn deleting_a_pull_request_cascades_its_review_comments() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_review_comment(&db, "c1", "pr1", "alice", "please fix the typo");

        {
            let conn = db.0.lock().unwrap();
            conn.execute("DELETE FROM pull_requests WHERE id = 'pr1'", []).unwrap();
        }

        let conn = db.0.lock().unwrap();
        let remaining_prs: i64 = conn.query_row("SELECT COUNT(*) FROM pull_requests", [], |r| r.get(0)).unwrap();
        let remaining_comments: i64 = conn.query_row("SELECT COUNT(*) FROM pr_review_comments", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining_prs, 0);
        assert_eq!(remaining_comments, 0, "comments must cascade-delete with their PR, not linger as orphans");
    }

    #[test]
    fn fetch_comments_by_ids_returns_only_matching_rows() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_review_comment(&db, "c1", "pr1", "alice", "please fix the typo");
        insert_review_comment(&db, "c2", "pr1", "bob", "one more thing");

        let fetched = fetch_comments_by_ids(&db, &["c1".to_string(), "missing".to_string()]).unwrap();
        assert_eq!(fetched.len(), 1);
        assert_eq!(fetched[0].author, "alice");
    }

    // -- other people's PRs -------------------------------------------------

    #[test]
    fn a_pr_is_mine_only_when_its_author_is_the_logged_in_user() {
        assert_eq!(classify_pr(Some("killua15"), Some("killua15"), false, false), "mine");
        assert_eq!(classify_pr(Some("alice"), Some("killua15"), false, false), "other");
    }

    #[test]
    fn a_pr_asking_for_my_review_or_assigned_to_me_lands_in_the_review_tab() {
        assert_eq!(classify_pr(Some("alice"), Some("killua15"), true, false), "review");
        assert_eq!(classify_pr(Some("alice"), Some("killua15"), false, true), "review");
    }

    /// Being *asked* to review your own PR is a real GitHub state (a
    /// co-author's request), but it's still your PR — the tab that polls for
    /// comments is where it belongs.
    #[test]
    fn my_own_pr_stays_mine_even_when_a_review_is_requested_from_me() {
        assert_eq!(classify_pr(Some("killua15"), Some("killua15"), true, true), "mine");
    }

    /// With `gh api user` failing there's no way to know which PRs are yours.
    /// Guessing would put strangers' PRs in the tab that gets polled and
    /// badged, so nothing is claimed as `mine`.
    #[test]
    fn without_a_known_login_nothing_is_claimed_as_mine() {
        assert_eq!(classify_pr(Some("alice"), None, false, false), "other");
        assert_eq!(classify_pr(Some("killua15"), None, false, false), "other");
        assert_eq!(classify_pr(Some("alice"), None, true, false), "review", "GitHub's own review-requested answer still counts");
    }

    /// The whole point: a PR with no session behind it reaches the screen,
    /// carrying who opened it and which tab it belongs to.
    #[test]
    fn the_inbox_lists_a_pr_nobody_here_opened() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_someone_elses_pr(&db, "proj1", "pr2", 2, "alice", "review");

        let inbox = list_pr_review_inbox_impl(&db).unwrap();
        assert_eq!(inbox.len(), 1, "both PRs belong to the same project");
        let theirs = inbox[0].entries.iter().find(|e| e.pull_request.id == "pr2").expect("a session-less PR must still be listed");
        assert_eq!(theirs.pull_request.author_login.as_deref(), Some("alice"));
        assert_eq!(theirs.pull_request.relation, "review");
        assert_eq!(theirs.session_id, None, "there is no session behind someone else's PR, and the entry must say so");
        assert_eq!(theirs.session_name, None);
    }

    /// The cost guard. Discovery now brings in every open PR of the repo;
    /// polling them all for comments would be dozens of `gh` calls every five
    /// minutes for PRs whose comments aren't the user's to answer.
    #[test]
    fn only_my_own_prs_are_polled_for_comments() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_someone_elses_pr(&db, "proj1", "pr2", 2, "alice", "review");
        insert_someone_elses_pr(&db, "proj1", "pr3", 3, "bob", "other");

        let conn = db.0.lock().unwrap();
        let checked: Vec<String> = tracked_open_prs_for_repo(&conn, "killua15", "Hangar").into_iter().map(|(id, _)| id).collect();
        assert_eq!(checked, vec!["pr1".to_string()]);
    }

    /// Nothing polls other people's PRs, so discovery's own list is what
    /// retires them: a merged PR of someone else's must stop being listed.
    #[test]
    fn a_foreign_pr_that_is_no_longer_open_is_dropped_on_the_next_discovery() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_someone_elses_pr(&db, "proj1", "pr2", 2, "alice", "review");
        insert_someone_elses_pr(&db, "proj1", "pr3", 3, "bob", "other");

        let conn = db.0.lock().unwrap();
        // GitHub still lists #3; #2 has been merged since the last pass.
        prune_closed_foreign_prs(&conn, "killua15", "Hangar", &[3]).unwrap();

        let left: Vec<String> = conn
            .prepare("SELECT id FROM pull_requests ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(left, vec!["pr1".to_string(), "pr3".to_string()], "only the merged foreign PR goes");
    }

    /// Your own PRs have their own lifecycle (checked every pass, deleted on
    /// merge). Discovery must not delete one just because it fell outside the
    /// repo's open list for a moment.
    #[test]
    fn pruning_never_touches_your_own_prs() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        let conn = db.0.lock().unwrap();
        prune_closed_foreign_prs(&conn, "killua15", "Hangar", &[]).unwrap();

        let left: i64 = conn.query_row("SELECT COUNT(*) FROM pull_requests WHERE id = 'pr1'", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1);
    }

    /// At the limit the list is a page, not the whole truth — pruning against
    /// it would churn perfectly open PRs in and out on every pass.
    #[test]
    fn a_full_page_of_results_prunes_nothing() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_someone_elses_pr(&db, "proj1", "pr2", 2, "alice", "review");

        let conn = db.0.lock().unwrap();
        let full_page: Vec<i64> = (100..100 + DISCOVERY_PR_LIMIT as i64).collect();
        prune_closed_foreign_prs(&conn, "killua15", "Hangar", &full_page).unwrap();

        let left: i64 = conn.query_row("SELECT COUNT(*) FROM pull_requests WHERE id = 'pr2'", [], |r| r.get(0)).unwrap();
        assert_eq!(left, 1, "a truncated list is not evidence that anything closed");
    }

    #[test]
    fn the_review_prompt_names_the_pr_and_refuses_to_publish_anything() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_someone_elses_pr(&db, "proj1", "pr2", 2, "alice", "review");
        let pr = fetch_pull_request_by_id(&db, "pr2").unwrap().unwrap();

        let prompt = format_pr_review_prompt(&pr);
        assert!(prompt.contains("#2"), "the agent must know which PR it's looking at");
        assert!(prompt.contains("alice"));
        assert!(prompt.contains("origin/main"), "a review is a diff against the base branch");
        assert!(prompt.contains("Do not push, comment, or post a review"), "posting on someone's PR is the user's to send, not the agent's");
    }

    /// The account chain, end to end: what the user picked wins; with nothing
    /// picked, the PR's own session decides; with no session, the project's
    /// most recent one.
    #[test]
    fn the_account_for_a_pr_prefers_the_users_choice_then_the_session_then_the_project() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
                 VALUES ('acc2','claude','sub','Other','work','','','acc2','healthy','now','now')",
                [],
            )
            .unwrap();
        }
        let mine = fetch_pull_request_by_id(&db, "pr1").unwrap().unwrap();
        assert_eq!(account_for_pr(&db, &mine, Some("acc2".to_string())).unwrap(), "acc2");
        assert_eq!(account_for_pr(&db, &mine, None).unwrap(), "acc1", "my own PR's session already answers this");

        insert_someone_elses_pr(&db, "proj1", "pr2", 2, "alice", "review");
        let theirs = fetch_pull_request_by_id(&db, "pr2").unwrap().unwrap();
        assert_eq!(account_for_pr(&db, &theirs, None).unwrap(), "acc1", "no session of its own — fall back to the project's most recent");
        assert_eq!(account_for_pr(&db, &theirs, Some("acc2".to_string())).unwrap(), "acc2");
    }

    /// A project that has never run a session here has no account to inherit,
    /// and a worktree session can't be created without one — so this must say
    /// so rather than fail somewhere deeper with a foreign-key error.
    #[test]
    fn a_pr_on_a_project_with_no_sessions_asks_for_an_account_instead_of_guessing() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        insert_someone_elses_pr(&db, "proj1", "pr2", 2, "alice", "other");
        {
            let conn = db.0.lock().unwrap();
            conn.execute("DELETE FROM pull_requests WHERE id = 'pr1'", []).unwrap();
            conn.execute("DELETE FROM sessions", []).unwrap();
        }
        let theirs = fetch_pull_request_by_id(&db, "pr2").unwrap().unwrap();

        let err = account_for_pr(&db, &theirs, None).unwrap_err();
        assert!(err.contains("pick one"), "the message has to point at the fix: {err}");
        assert_eq!(account_for_pr(&db, &theirs, Some("acc1".to_string())).unwrap(), "acc1");
    }

    #[test]
    fn resolve_session_for_pr_branch_reuses_anchor_session_when_branch_matches() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            conn.execute("UPDATE sessions SET worktree_branch = 'feat/x' WHERE id = 'session-pr1'", []).unwrap();
            conn.execute("UPDATE pull_requests SET head_branch = 'feat/x' WHERE id = 'pr1'", []).unwrap();
        }
        let pr = fetch_pull_request_by_id(&db, "pr1").unwrap().unwrap();

        let app = tauri::test::mock_app();
        let session_id = resolve_session_for_pr_branch(app.handle(), &db, &pr, "acc1").expect("should resolve without any gh/git calls");
        assert_eq!(session_id, "session-pr1", "the anchor session is already on the right branch — no need to create anything");
    }

    #[test]
    fn resolve_session_for_pr_branch_reuses_an_existing_session_already_on_that_branch() {
        // `setup_db_with_open_pr` gives pr1 number=1, so the deterministic
        // local review branch a previous "Send to agent" would have created
        // *if the PR's real branch name was already taken at the time* is
        // "fleet/pr-1-review" — deliberately the *pre-rename* name, so this
        // also covers the legacy branch still being reused after the rename
        // instead of a second worktree being cut for the same PR. The reuse
        // lookup must find a session on
        // this fallback name too, not just the PR's real `head_branch`.
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at, worktree_branch)
                 VALUES ('other-session','review session','proj1','acc1','sonnet','high','ask_edits','waiting','now','fleet/pr-1-review')",
                [],
            )
            .unwrap();
            conn.execute("UPDATE pull_requests SET head_branch = 'feat/x' WHERE id = 'pr1'", []).unwrap();
        }
        let pr = fetch_pull_request_by_id(&db, "pr1").unwrap().unwrap();

        let app = tauri::test::mock_app();
        let session_id = resolve_session_for_pr_branch(app.handle(), &db, &pr, "acc1").expect("should resolve without any gh/git calls");
        assert_eq!(session_id, "other-session", "an existing session already on the PR's branch must be reused, not recreated");
    }

    /// End-to-end against a real local repo: nothing exists yet for this
    /// PR's branch anywhere locally (only on `origin`, as it would be for a
    /// real PR nobody's checked out yet), so this must actually fetch the
    /// branch and create a real linked worktree on it — checked out on the
    /// PR's own real branch name, since nothing local conflicts with it.
    #[test]
    fn resolve_session_for_pr_branch_creates_a_real_worktree_when_nothing_exists() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("resolve-session");
        git_capture(&work_dir, &["checkout", "-b", "feat/pr-branch"]).unwrap();
        std::fs::write(work_dir.join("pr_file.txt"), "hello from the PR branch").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "pr commit"]).unwrap();
        git_capture(&work_dir, &["push", "-u", "origin", "feat/pr-branch"]).unwrap();
        git_capture(&work_dir, &["checkout", "main"]).unwrap();
        // Delete the local ref too — real-world, the PR's branch only
        // exists on `origin` until someone actually checks it out; leaving
        // it here would make this test's own setup the thing that "already
        // has it locally," defeating the "nothing exists" scenario.
        git_capture(&work_dir, &["branch", "-D", "feat/pr-branch"]).unwrap();

        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            conn.execute("UPDATE projects SET path = ?1 WHERE id = 'proj1'", params![work_dir.to_string_lossy().to_string()]).unwrap();
            conn.execute("UPDATE pull_requests SET head_branch = 'feat/pr-branch' WHERE id = 'pr1'", []).unwrap();
        }
        let pr = fetch_pull_request_by_id(&db, "pr1").unwrap().unwrap();

        let app = tauri::test::mock_app();
        app.manage(db);
        let db_state = app.state::<DbConnection>();
        let new_session_id =
            resolve_session_for_pr_branch(app.handle(), &db_state, &pr, "acc1").expect("should fetch the branch for real and create a linked worktree");
        assert_ne!(new_session_id, "session-pr1", "the anchor session isn't on this branch, so a new one must be created");

        let (worktree_path, worktree_branch, worktree_kind, parent_session_id): (String, String, String, Option<String>) = {
            let conn = db_state.0.lock().unwrap();
            conn.query_row(
                "SELECT worktree_path, worktree_branch, worktree_kind, parent_session_id FROM sessions WHERE id = ?1",
                params![new_session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap()
        };
        assert_eq!(worktree_branch, "feat/pr-branch", "nothing local conflicts with the PR's real branch name, so it should be used as-is");
        assert_eq!(worktree_kind, "linked");
        assert_eq!(parent_session_id.as_deref(), Some("session-pr1"));
        assert!(Path::new(&worktree_path).join("pr_file.txt").is_file(), "the new worktree must actually be checked out on the PR's real branch content");
        let gitignore = std::fs::read_to_string(work_dir.join(".gitignore")).unwrap_or_default();
        assert!(gitignore.contains(".hangar/"), "the project's .gitignore must cover the worktree folder so it's not untracked clutter");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    /// The failure a user hit twice in a row: the worktree directory was gone,
    /// the session row wasn't, and "Send to terminal" handed back a session
    /// whose working directory didn't exist — so the terminal refused with
    /// "…/worktrees/pr-103 no longer exists on disk".
    ///
    /// The row and the directory are separate facts, and only one of them is
    /// deleted when someone removes a worktree by hand.
    #[test]
    fn a_session_whose_worktree_was_deleted_is_not_reused() {
        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            // A session pointing at a directory that doesn't exist — exactly
            // what's left behind after deleting a worktree from the Git panel.
            conn.execute("UPDATE sessions SET worktree_path = '/nope/gone/pr-103' WHERE id = 'session-pr1'", []).unwrap();
        }
        assert!(!session_worktree_is_on_disk(&db, "session-pr1"), "a missing directory must not count as a usable worktree");

        // And a session with no worktree at all runs in the project root, which
        // exists by definition — that one is still fine to reuse.
        {
            let conn = db.0.lock().unwrap();
            conn.execute("UPDATE sessions SET worktree_path = NULL WHERE id = 'session-pr1'", []).unwrap();
        }
        assert!(session_worktree_is_on_disk(&db, "session-pr1"));
    }

    /// The regression a user hit: "Send to terminal" on PR #103 produced a
    /// worktree on `hangar/pr-103-review` even though the PR's own branch was
    /// sitting right there, unused — so commits went to a branch GitHub Desktop
    /// reported as something other than the PR.
    ///
    /// The cause was testing "does this branch exist locally" when the thing
    /// git actually refuses is "is it checked out somewhere else".
    #[test]
    fn a_pr_worktree_checks_out_the_prs_own_branch_when_it_exists_and_is_free() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("pr-branch-reuse");
        git_capture(&work_dir, &["checkout", "-b", "feat/pr-branch"]).unwrap();
        std::fs::write(work_dir.join("pr_file.txt"), "from the PR").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "pr commit"]).unwrap();
        git_capture(&work_dir, &["push", "-u", "origin", "feat/pr-branch"]).unwrap();
        // Back to main, but *leave the branch in place* — the state the bug
        // needed, and the ordinary state after fetching a colleague's PR.
        git_capture(&work_dir, &["checkout", "main"]).unwrap();

        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            conn.execute("UPDATE projects SET path = ?1 WHERE id = 'proj1'", params![work_dir.to_string_lossy().to_string()]).unwrap();
            conn.execute("UPDATE pull_requests SET head_branch = 'feat/pr-branch' WHERE id = 'pr1'", []).unwrap();
        }
        let pr = fetch_pull_request_by_id(&db, "pr1").unwrap().unwrap();

        let app = tauri::test::mock_app();
        app.manage(db);
        let db_state = app.state::<DbConnection>();
        let session_id = resolve_session_for_pr_branch(app.handle(), &db_state, &pr, "acc1").expect("an existing, free branch must just be checked out");

        let branch: String = {
            let conn = db_state.0.lock().unwrap();
            conn.query_row("SELECT worktree_branch FROM sessions WHERE id = ?1", params![session_id], |row| row.get(0)).unwrap()
        };
        assert_eq!(branch, "feat/pr-branch", "the worktree belongs on the PR's branch, not on a name Hangar made up");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    /// The exact bug reported live: the PR's real branch is already checked
    /// out *somewhere* (here, a second worktree of the same repo — in
    /// practice this was the user's own separate clone of the project,
    /// actively open elsewhere). `git fetch origin <branch>:<branch>` fails
    /// outright in that situation ("refusing to fetch into branch ...
    /// checked out at ..."). Confirms the fix: fetching into a remote-
    /// tracking ref and creating our own distinctly-named local branch
    /// sidesteps the conflict entirely, regardless of where else that
    /// branch happens to be checked out.
    #[test]
    fn resolve_session_for_pr_branch_succeeds_even_when_the_real_branch_is_checked_out_elsewhere() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("resolve-session-collision");
        git_capture(&work_dir, &["checkout", "-b", "feat/pr-branch"]).unwrap();
        std::fs::write(work_dir.join("pr_file.txt"), "hello from the PR branch").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "pr commit"]).unwrap();
        git_capture(&work_dir, &["push", "-u", "origin", "feat/pr-branch"]).unwrap();
        git_capture(&work_dir, &["checkout", "main"]).unwrap();

        // Simulate the real-world collision: some other worktree of this
        // same repo has the PR's branch checked out right now.
        let other_checkout = make_temp_dir("resolve-session-collision-other-checkout");
        std::fs::remove_dir(&other_checkout).unwrap(); // `worktree add` wants to create this itself
        git_capture(&work_dir, &["worktree", "add", &other_checkout.to_string_lossy(), "feat/pr-branch"]).unwrap();

        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            conn.execute("UPDATE projects SET path = ?1 WHERE id = 'proj1'", params![work_dir.to_string_lossy().to_string()]).unwrap();
            conn.execute("UPDATE pull_requests SET head_branch = 'feat/pr-branch' WHERE id = 'pr1'", []).unwrap();
        }
        let pr = fetch_pull_request_by_id(&db, "pr1").unwrap().unwrap();

        let app = tauri::test::mock_app();
        app.manage(db);
        let db_state = app.state::<DbConnection>();
        let new_session_id = resolve_session_for_pr_branch(app.handle(), &db_state, &pr, "acc1")
            .expect("must succeed even though feat/pr-branch is checked out in another worktree");
        assert_ne!(new_session_id, "session-pr1");

        let worktree_branch: String = {
            let conn = db_state.0.lock().unwrap();
            conn.query_row("SELECT worktree_branch FROM sessions WHERE id = ?1", params![new_session_id], |row| row.get(0)).unwrap()
        };
        assert_eq!(worktree_branch, "hangar/pr-1-review", "the real branch name is taken (exists locally), so it must fall back to Hangar's own name");
        let gitignore = std::fs::read_to_string(work_dir.join(".gitignore")).unwrap_or_default();
        assert!(gitignore.contains(".hangar/"), "the project's .gitignore must cover the worktree folder so it's not untracked clutter");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&other_checkout);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    /// The second half of the deleted-worktree bug, and the one that actually
    /// bit: pruning the registry lets `worktree add` reuse the *path*, but the
    /// review *branch* is still there — deleting a directory doesn't delete a
    /// ref. So the retry died on "a branch named 'hangar/pr-1-review' already
    /// exists", forever, with no way out from the UI.
    ///
    /// Confirms it now attaches to the leftover branch instead of trying to
    /// create it, and fast-forwards it to whatever the PR has gained since.
    #[test]
    fn resolve_session_for_pr_branch_reuses_a_leftover_review_branch_whose_worktree_is_gone() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("resolve-session-leftover-branch");
        git_capture(&work_dir, &["checkout", "-b", "feat/pr-branch"]).unwrap();
        std::fs::write(work_dir.join("pr_file.txt"), "hello from the PR branch").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "pr commit"]).unwrap();
        git_capture(&work_dir, &["push", "-u", "origin", "feat/pr-branch"]).unwrap();
        // The PR's own branch stays checked out here — the real situation, where
        // the user has the project itself open on it — which is what forces the
        // fallback name in the first place.

        // A previous review worktree existed and its directory was deleted by
        // hand. Git's registry gets pruned; the branch does not.
        let stale = make_temp_dir("resolve-session-leftover-branch-stale");
        std::fs::remove_dir(&stale).unwrap();
        git_capture(&work_dir, &["worktree", "add", "-b", "hangar/pr-1-review", &stale.to_string_lossy(), "origin/feat/pr-branch"]).unwrap();
        std::fs::remove_dir_all(&stale).unwrap();

        // …and the PR moved on afterwards.
        std::fs::write(work_dir.join("pr_file.txt"), "a later commit on the PR").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "later pr commit"]).unwrap();
        git_capture(&work_dir, &["push", "origin", "feat/pr-branch"]).unwrap();

        let db = setup_db_with_open_pr("proj1", "Project One", "pr1");
        {
            let conn = db.0.lock().unwrap();
            conn.execute("UPDATE projects SET path = ?1 WHERE id = 'proj1'", params![work_dir.to_string_lossy().to_string()]).unwrap();
            conn.execute("UPDATE pull_requests SET head_branch = 'feat/pr-branch' WHERE id = 'pr1'", []).unwrap();
        }
        let pr = fetch_pull_request_by_id(&db, "pr1").unwrap().unwrap();

        let app = tauri::test::mock_app();
        app.manage(db);
        let db_state = app.state::<DbConnection>();
        let new_session_id = resolve_session_for_pr_branch(app.handle(), &db_state, &pr, "acc1")
            .expect("must succeed even though hangar/pr-1-review is left over from a deleted worktree");

        let (branch, path): (String, String) = {
            let conn = db_state.0.lock().unwrap();
            conn.query_row("SELECT worktree_branch, worktree_path FROM sessions WHERE id = ?1", params![new_session_id], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap()
        };
        assert_eq!(branch, "hangar/pr-1-review", "the leftover branch must be reused, not renamed around");
        assert_eq!(
            std::fs::read_to_string(Path::new(&path).join("pr_file.txt")).unwrap(),
            "a later commit on the PR",
            "a reused review branch must be fast-forwarded to the PR's current tip"
        );

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-test-pr-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Real bare repo as `origin` + a real working clone with an initial
    /// commit on `main`, `origin/HEAD` pointed at `main` so
    /// `detect_default_branch` resolves it without ever needing `gh`.
    fn setup_real_repo_with_remote(label: &str) -> (PathBuf, PathBuf) {
        let remote_dir = make_temp_dir(&format!("{label}-remote"));
        git_capture(&remote_dir, &["init", "--bare"]).unwrap();

        let work_dir = make_temp_dir(&format!("{label}-work"));
        git_capture(&work_dir, &["init"]).unwrap();
        git_capture(&work_dir, &["checkout", "-b", "main"]).unwrap();
        git_capture(&work_dir, &["config", "user.email", "hangar-test@example.com"]).unwrap();
        git_capture(&work_dir, &["config", "user.name", "Hangar Test"]).unwrap();
        std::fs::write(work_dir.join("README.md"), "hello").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "init"]).unwrap();
        git_capture(&work_dir, &["remote", "add", "origin", remote_dir.to_str().unwrap()]).unwrap();
        git_capture(&work_dir, &["push", "-u", "origin", "main"]).unwrap();
        git_capture(&work_dir, &["remote", "set-head", "origin", "main"]).unwrap();

        (remote_dir, work_dir)
    }

    #[test]
    fn detect_default_branch_resolves_via_origin_head() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("default-branch");
        assert_eq!(detect_default_branch(&work_dir), "main");
        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn prepare_branch_for_pr_creates_a_new_branch_when_starting_on_base() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("new-branch");
        std::fs::write(work_dir.join("feature.txt"), "new stuff").unwrap();

        let (branch, base) = prepare_branch_for_pr(&work_dir, "My Cool Session", &[]).expect("prepare_branch_for_pr should succeed");
        assert_eq!(base, "main");
        assert_eq!(branch, "hangar/my-cool-session");

        let current = git_capture(&work_dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(current, "hangar/my-cool-session", "the working dir must actually be on the new branch");

        let remote_branches = git_capture(&remote_dir, &["branch", "--list", "hangar/my-cool-session"]).unwrap();
        assert!(remote_branches.contains("hangar/my-cool-session"), "the branch must be pushed to origin");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    /// A worktree checked out from a remote ref directly (`git worktree add
    /// <dir> origin/main`, exactly what a detached test/CI checkout looks
    /// like) leaves `git rev-parse --abbrev-ref HEAD` reporting the literal
    /// string `"HEAD"`, not a real branch name — pushing under that name
    /// fails outright. Caught by a real end-to-end run against GitHub;
    /// this pins the fix against a local remote instead.
    #[test]
    fn prepare_branch_for_pr_creates_a_branch_when_starting_in_detached_head() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("detached-head");
        let head_commit = git_capture(&work_dir, &["rev-parse", "HEAD"]).unwrap();
        git_capture(&work_dir, &["checkout", &head_commit]).unwrap();
        assert_eq!(git_capture(&work_dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(), "HEAD", "must actually be detached for this test to mean anything");
        std::fs::write(work_dir.join("scratch.txt"), "detached head content").unwrap();

        let (branch, base) = prepare_branch_for_pr(&work_dir, "Detached Session", &[]).expect("prepare_branch_for_pr must handle a detached HEAD, not try to push it literally");
        assert_eq!(base, "main");
        assert_eq!(branch, "hangar/detached-session");

        let remote_branches = git_capture(&remote_dir, &["branch", "--list", "hangar/detached-session"]).unwrap();
        assert!(remote_branches.contains("hangar/detached-session"), "the new branch must be pushed to origin");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn prepare_branch_for_pr_reuses_an_already_checked_out_non_base_branch() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("existing-branch");
        git_capture(&work_dir, &["checkout", "-b", "already-working-here"]).unwrap();
        std::fs::write(work_dir.join("more.txt"), "more stuff").unwrap();

        let (branch, base) = prepare_branch_for_pr(&work_dir, "irrelevant name", &[]).expect("prepare_branch_for_pr should succeed");
        assert_eq!(base, "main");
        assert_eq!(branch, "already-working-here", "must not create a new branch when already off base");

        let remote_branches = git_capture(&remote_dir, &["branch", "--list", "already-working-here"]).unwrap();
        assert!(remote_branches.contains("already-working-here"));

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn prepare_branch_for_pr_is_a_no_op_commit_when_tree_is_already_clean() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("clean-tree");
        git_capture(&work_dir, &["checkout", "-b", "already-clean"]).unwrap();
        // No uncommitted changes at all.
        let (branch, _base) = prepare_branch_for_pr(&work_dir, "irrelevant", &[]).expect("prepare_branch_for_pr should succeed even with nothing to commit");
        assert_eq!(branch, "already-clean");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }
}
