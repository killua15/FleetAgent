//! Real PR creation + review-comment polling via the `gh` CLI — Fleet has no
//! GitHub App/webhook (it's a desktop app with no public endpoint), so this
//! is the same shape Conductor uses: poll while the app is open, plus a
//! manual "check now" escape hatch. Never auto-sends new comments to an
//! agent on its own — `check_pr_comments`/the poller only ever *surface*
//! them; `send_pr_comments_to_session` is a separate, explicit action the
//! user triggers from the UI.

use crate::commands::sessions::{ensure_fleet_dir_ignored, slugify, unique_worktree_dir};
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
    let mut cmd = Command::new(providers::resolve_binary("git"));
    cmd.args(args).current_dir(cwd);
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
fn gh_output(cwd: Option<&Path>, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new(providers::resolve_binary("gh"));
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    crate::process_timeout::run_with_timeout(cmd, crate::process_timeout::DEFAULT_TIMEOUT).map_err(|e| format!("gh {}: {e}", args.join(" ")))
}

/// Clear, actionable error instead of letting a bare `gh pr create` failure
/// (missing binary or expired login) surface as a cryptic exit-code message.
fn gh_available_and_authed() -> Result<(), String> {
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
fn local_branch_exists(cwd: &Path, branch: &str) -> bool {
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
    body.push_str("_Opened by Fleet._");
    body
}

fn generate_pr_title_and_body(cwd: &Path, base: &str, branch: &str, session_name: &str) -> Result<(String, String), String> {
    let name_status = git_capture(cwd, &["diff", "--name-status", &format!("{base}...{branch}")])?;
    let (added, modified, removed) = group_name_status(&name_status);
    Ok((session_name.to_string(), render_pr_body(&added, &modified, &removed)))
}

fn row_to_pull_request(row: &rusqlite::Row) -> rusqlite::Result<PullRequestInfo> {
    Ok(PullRequestInfo {
        id: row.get(0)?,
        session_id: row.get(1)?,
        repo_owner: row.get(2)?,
        repo_name: row.get(3)?,
        number: row.get(4)?,
        url: row.get(5)?,
        state: row.get(6)?,
        last_comment_seen_at: row.get(7)?,
        created_at: row.get(8)?,
        last_check_error: row.get(9)?,
        last_seen_updated_at: row.get(10)?,
        head_branch: row.get(11)?,
    })
}

/// The most recent PR for a session — correct only when a session has at
/// most one PR, which was true back when the only way to get a PR was
/// Fleet's own "Create PR" button. `discover_open_prs_for_project` broke
/// that assumption: every PR discovery anchors to "the project's most
/// recent session" purely as an FK target, so a project with several open
/// PRs and one session ends up with several `pull_requests` rows sharing
/// that same `session_id`. Anything keyed by session_id here (rather than
/// the PR's own id) would silently see only the newest of them, forever —
/// use `fetch_pull_request_by_id` for anything that must reach every PR.
fn fetch_pull_request(db: &DbConnection, session_id: &str) -> Result<Option<PullRequestInfo>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, session_id, repo_owner, repo_name, number, url, state, last_comment_seen_at, created_at, last_check_error, last_seen_updated_at, head_branch
         FROM pull_requests WHERE session_id = ?1 ORDER BY created_at DESC LIMIT 1",
        params![session_id],
        row_to_pull_request,
    )
    .optional()
    .map_err(|e| e.to_string())
}

fn fetch_pull_request_by_id(db: &DbConnection, pr_id: &str) -> Result<Option<PullRequestInfo>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT id, session_id, repo_owner, repo_name, number, url, state, last_comment_seen_at, created_at, last_check_error, last_seen_updated_at, head_branch
         FROM pull_requests WHERE id = ?1",
        params![pr_id],
        row_to_pull_request,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Ensures `cwd` is on a pushed, PR-ready branch: if it's currently sitting
/// on the base branch (or not on any real branch at all — a detached HEAD,
/// which `git rev-parse --abbrev-ref HEAD` reports as the literal string
/// `"HEAD"` and which `git push` can't publish under that name), creates
/// and checks out a new one (`fleet/<slug>`); commits anything uncommitted
/// (no stage/commit UI exists yet, so this is a single blanket commit);
/// pushes to `origin`. Returns `(branch, base)`. Pure git, no `gh` calls, so
/// it's testable against a real local remote without touching GitHub.
fn prepare_branch_for_pr(cwd: &Path, session_name: &str) -> Result<(String, String), String> {
    let base = detect_default_branch(cwd);
    let current_branch = git_capture(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let branch = if current_branch == base || current_branch == "HEAD" {
        let new_branch = format!("fleet/{}", slugify(session_name));
        git_capture(cwd, &["checkout", "-b", &new_branch])?;
        new_branch
    } else {
        current_branch
    };

    let status = git_capture(cwd, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        git_capture(cwd, &["add", "-A"])?;
        git_capture(cwd, &["commit", "-m", &format!("Changes from Fleet session: {session_name}")])?;
    }

    git_capture(cwd, &["push", "-u", "origin", &branch])?;
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

        let (branch, base) = prepare_branch_for_pr(cwd_path, &session_name)?;

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
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO pull_requests (id, session_id, repo_owner, repo_name, number, url, state, last_comment_seen_at, created_at, head_branch)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'open', NULL, ?7, ?8)",
                params![id, session_id, owner, repo, number, url, now, branch],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(PullRequestInfo {
            id,
            session_id,
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
/// history can grow a source of comments Fleet couldn't previously fetch
/// (inline diff comments were invisible before this was added) — if
/// "already surfaced" were tracked as "newer than the latest timestamp seen
/// so far", any such backfilled source would stay permanently invisible the
/// moment a single newer comment from any other source had already advanced
/// that watermark. Identity-based dedup has no such blind spot: a comment
/// is new exactly when Fleet has never persisted its id before.
#[derive(Debug, Clone, PartialEq)]
struct RawPrComment {
    external_id: String,
    author: String,
    body: String,
    created_at: String,
    path: Option<String>,
    line: Option<i64>,
}

/// Normalizes `gh pr view --json comments,reviews`'s two differently-shaped
/// arrays into one list, sorted oldest-first, dropping review submissions
/// with no comment text (a bare "approve"/"request changes" with nothing to
/// act on). Each entry already carries GitHub's own `id` by default (no
/// extra `--json` field needed — it's part of gh's fixed shape for both
/// arrays). Neither shape carries file/line context — that's only on inline
/// diff comments, parsed separately by `parse_inline_comments_json`.
fn parse_pr_comments_json(json: &Value) -> Vec<RawPrComment> {
    let mut out = Vec::new();
    if let Some(comments) = json.get("comments").and_then(|v| v.as_array()) {
        for c in comments {
            let body = c.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if body.trim().is_empty() {
                continue;
            }
            let external_id = c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let author = c.pointer("/author/login").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let created_at = c.get("createdAt").and_then(|v| v.as_str()).unwrap_or("").to_string();
            out.push(RawPrComment { external_id, author, body, created_at, path: None, line: None });
        }
    }
    if let Some(reviews) = json.get("reviews").and_then(|v| v.as_array()) {
        for r in reviews {
            let body = r.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if body.trim().is_empty() {
                continue;
            }
            let external_id = r.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let author = r.pointer("/author/login").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let created_at = r.get("submittedAt").and_then(|v| v.as_str()).unwrap_or("").to_string();
            out.push(RawPrComment { external_id, author, body, created_at, path: None, line: None });
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

/// Normalizes `gh api repos/{owner}/{repo}/pulls/{n}/comments`'s array of
/// inline diff comments — the review comments a plain `gh pr view
/// --json comments,reviews` can't see at all, since GitHub keeps line-level
/// feedback in a separate REST resource from the PR's own comments/reviews.
/// Uses `original_line` as a fallback for `line` since GitHub nulls out
/// `line` once the commented line has scrolled out of the current diff.
fn parse_inline_comments_json(json: &Value) -> Vec<RawPrComment> {
    let mut out = Vec::new();
    if let Some(comments) = json.as_array() {
        for c in comments {
            let body = c.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if body.trim().is_empty() {
                continue;
            }
            let external_id = c.get("id").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default();
            let author = c.pointer("/user/login").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();
            let created_at = c.get("created_at").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let path = c.get("path").and_then(|v| v.as_str()).map(|s| s.to_string());
            let line = c.get("line").and_then(|v| v.as_i64()).or_else(|| c.get("original_line").and_then(|v| v.as_i64()));
            out.push(RawPrComment { external_id, author, body, created_at, path, line });
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

/// Real `gh api` call for inline diff comments on one PR — the counterpart
/// to `gh pr view --json comments,reviews` that actually sees file/line
/// feedback (e.g. a review left only as inline comments, with no top-level
/// summary body, which `parse_pr_comments_json` would otherwise drop
/// entirely). `--paginate` merges multi-page results into one JSON array.
fn fetch_inline_review_comments(repo: &str, pr_number: i64) -> Result<Vec<RawPrComment>, String> {
    let output = gh_output(None, &["api", &format!("repos/{repo}/pulls/{pr_number}/comments"), "--paginate"])?;
    if !output.status.success() {
        return Err(format!("gh api pulls/comments failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    Ok(parse_inline_comments_json(&json))
}

/// The `gh`-authenticated user's own login, so their own follow-up comments
/// on a PR Fleet opened never show up as "review feedback to act on" — best
/// effort, `None` (no filtering) if `gh api user` fails for any reason.
fn fetch_gh_login() -> Option<String> {
    let output = gh_output(None, &["api", "user", "--jq", ".login"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let login = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!login.is_empty()).then_some(login)
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

/// Cheap freshness probe — just state + GitHub's own `updatedAt`, no comment
/// bodies at all. `check_pr_comments_for` runs this first so a PR with
/// nothing new (the common case once a project's been checked a few times)
/// costs one small `gh` call instead of a full `gh pr view --json
/// comments,reviews` plus a separate REST call for inline comments.
fn fetch_pr_state_and_updated_at(pr: &PullRequestInfo) -> Result<(String, String), String> {
    let repo = format!("{}/{}", pr.repo_owner, pr.repo_name);
    let output = gh_output(None, &["pr", "view", &pr.number.to_string(), "--repo", &repo, "--json", "state,updatedAt"])?;
    if !output.status.success() {
        return Err(format!("gh pr view failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let state = json.get("state").and_then(|v| v.as_str()).unwrap_or("OPEN").to_lowercase();
    let updated_at = json.get("updatedAt").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Ok((state, updated_at))
}

/// Real `gh` fetch for one PR — general comments/reviews plus inline diff
/// comments, combined into one list. Only called once the cheap probe above
/// has decided something might actually be new.
fn fetch_pr_comments(pr: &PullRequestInfo) -> Result<Vec<RawPrComment>, String> {
    let repo = format!("{}/{}", pr.repo_owner, pr.repo_name);
    let output = gh_output(None, &["pr", "view", &pr.number.to_string(), "--repo", &repo, "--json", "comments,reviews"])?;
    if !output.status.success() {
        return Err(format!("gh pr view failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;

    // General comments/reviews plus inline diff comments — two separate `gh`
    // resources GitHub keeps apart, combined here so the rest of this
    // function (dedup, self-filter, persistence) treats them identically. A
    // PR with an inline-only review (no top-level summary text) would
    // otherwise show nothing at all.
    let mut all_comments = parse_pr_comments_json(&json);
    all_comments.extend(fetch_inline_review_comments(&repo, pr.number)?);
    Ok(all_comments)
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

fn check_pr_comments_for_pr_id(db: &DbConnection, pr_id: &str) -> Result<Vec<PrComment>, String> {
    let pr = fetch_pull_request_by_id(db, pr_id)?.ok_or_else(|| "no such PR".to_string())?;
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
    let (state, updated_at) = match fetch_pr_state_and_updated_at(&pr) {
        Ok(result) => result,
        Err(e) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute("UPDATE pull_requests SET last_check_error = ?2 WHERE id = ?1", params![pr.id, e]).map_err(|e| e.to_string())?;
            return Ok(vec![]);
        }
    };

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

    let all_comments = match fetch_pr_comments(&pr) {
        Ok(comments) => comments,
        Err(e) => {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute("UPDATE pull_requests SET last_check_error = ?2 WHERE id = ?1", params![pr.id, e]).map_err(|e| e.to_string())?;
            return Ok(vec![]);
        }
    };

    let self_login = fetch_gh_login();

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
        "SELECT pr.id, pr.session_id, pr.repo_owner, pr.repo_name, pr.number, pr.url, pr.state, pr.last_comment_seen_at, pr.created_at, pr.last_check_error, pr.last_seen_updated_at, pr.head_branch
         FROM pull_requests pr JOIN pr_review_comments c ON c.pull_request_id = pr.id
         WHERE c.id = ?1",
        params![comment_id],
        row_to_pull_request,
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Finds (or creates) the session that should actually receive a PR's
/// review feedback — one genuinely checked out on that PR's real branch,
/// not just whatever session happens to be tied to the row (a project with
/// several *discovered* PRs shares one arbitrary anchor session — see
/// `fetch_pull_request`'s doc comment). Same shape Devin/Cursor/Copilot's
/// coding agents use: a dedicated worktree on the PR's own branch, so the
/// agent has the real diff and can push straight back to update it.
///
/// 1. If the anchor session is already on this exact branch (true for any
///    PR Fleet itself opened — it created that branch), reuse it, free.
/// 2. Else, if some other session in the project is already on this branch
///    (e.g. a previous "Send to agent" for this same PR already made one),
///    reuse that.
/// 3. Else, create a real subsession: fetch the branch for real and add a
///    worktree on it, exactly like `create_subsession` does for a brand new
///    branch — just checking out one that already exists instead.
fn resolve_session_for_pr_branch<R: Runtime>(app: &AppHandle<R>, db: &DbConnection, pr: &PullRequestInfo) -> Result<String, String> {
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

    let (project_id, project_path, anchor_account_id, anchor_model, anchor_effort, anchor_mode, anchor_worktree_branch): (
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
    ) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT p.id, p.path, s.account_id, s.model, s.effort, s.mode, s.worktree_branch
             FROM sessions s JOIN projects p ON p.id = s.project_id
             WHERE s.id = ?1",
            params![pr.session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
        )
        .map_err(|e| format!("anchor session lookup failed: {e}"))?
    };

    // A local branch by the same name as `head_branch` might already be
    // checked out *anywhere* touching this repo — the user's own primary
    // clone (very plausibly the exact case if they're actively working on
    // this PR outside Fleet), another Fleet worktree, anything. Git refuses
    // to check out the same local branch name in two places at once, so
    // this Fleet-owned fallback name (deterministic from the PR number, so
    // it's the same every time and safely re-discoverable in step 2) only
    // gets used when the real name turns out to be unsafe — see the
    // `local_branch_exists` check below, right before the worktree is
    // actually created.
    let local_review_branch = format!("fleet/pr-{}-review", pr.number);

    if anchor_worktree_branch.as_deref() == Some(head_branch.as_str()) || anchor_worktree_branch.as_deref() == Some(local_review_branch.as_str()) {
        return Ok(pr.session_id.clone());
    }

    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        // A previous "Send to agent" for this same PR could have landed on
        // either name, depending on whether there was a conflict at the
        // time — check both.
        if let Some(existing) = conn
            .query_row(
                "SELECT id FROM sessions WHERE project_id = ?1 AND worktree_branch IN (?2, ?3) ORDER BY started_at DESC LIMIT 1",
                params![project_id, head_branch, local_review_branch],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| e.to_string())?
        {
            return Ok(existing);
        }
    }

    let worktree_dir = unique_worktree_dir(&project_path, &format!("pr-{}", pr.number));
    let project_dir = Path::new(&project_path);
    // Same `.fleet/worktrees/...` location `create_subsession` uses — needs
    // the same gitignore safeguard so it doesn't show up as untracked
    // clutter (or worse, get accidentally staged) in the project's own
    // `git status` the first time this runs.
    ensure_fleet_dir_ignored(&project_path);

    // Fetching into a *remote-tracking* ref (no local branch name involved)
    // is never blocked by anything being checked out — only creating/moving
    // a local branch ref is. Prefer checking the worktree out on the PR's
    // own branch name when nothing local already claims that name anywhere
    // in the repo; only fall back to the Fleet-owned name when there'd be
    // an actual conflict, so the common case shows the PR's real branch
    // instead of always inventing one.
    git_capture(project_dir, &["fetch", "origin", &head_branch])?;
    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let chosen_branch = if local_branch_exists(project_dir, &head_branch) { local_review_branch.clone() } else { head_branch.clone() };
    git_capture(
        project_dir,
        &["worktree", "add", "-b", &chosen_branch, &worktree_dir.to_string_lossy(), &format!("origin/{head_branch}")],
    )?;
    // Track the real remote branch explicitly (rather than relying on
    // whatever `-b ... origin/<head_branch>` inferred) so a plain
    // `git push` from inside this worktree updates the actual PR either way.
    git_capture(&worktree_dir, &["branch", "--set-upstream-to", &format!("origin/{head_branch}"), &chosen_branch])?;

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
                anchor_account_id,
                anchor_model,
                anchor_effort,
                anchor_mode,
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
        let target_session_id = resolve_session_for_pr_branch(&app, &db, &pr)?;

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
/// PR's own branch (via `resolve_session_for_pr_branch`, unchanged) under the
/// project's account, running `claude '<the comments>'` so Claude Code opens
/// already working on them. This is the terminal-first replacement for
/// "Send to agent". Returns the opened terminal.
#[tauri::command]
pub async fn send_pr_comments_to_terminal(app: AppHandle, comment_ids: Vec<String>, cols: u16, rows: u16) -> Result<crate::terminals::TerminalInfo, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let comments = fetch_comments_by_ids(&db, &comment_ids)?;
        if comments.is_empty() {
            return Err("no matching comments to send".to_string());
        }
        let pr = fetch_pull_request_for_comment(&db, &comment_ids[0])?.ok_or_else(|| "comment has no linked PR".to_string())?;

        // Reuses the exact worktree-on-the-PR-branch resolution the session
        // path uses — the container session carries the worktree path and the
        // project's account, which the terminal then runs under.
        let target_session_id = resolve_session_for_pr_branch(&app, &db, &pr)?;
        let account_id: String = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT account_id FROM sessions WHERE id = ?1", params![target_session_id], |row| row.get(0))
                .map_err(|e| format!("could not read the PR worktree's account: {e}"))?
        };

        let prompt = to_single_line(&format_pr_comments_prompt(&comments));
        let info = crate::terminals::open_terminal(
            app.clone(),
            crate::terminals::TerminalTarget { kind: crate::terminals::TargetKind::Session, id: target_session_id },
            Some(account_id),
            Some(crate::terminals::InitCommand::Start),
            Some(prompt),
            cols,
            rows,
        )?;
        mark_comments_resolved(&db, &comment_ids, "sent")?;
        Ok(info)
    })
    .await?
}

/// Clears comments out of the inbox without sending anything to an agent —
/// for feedback that doesn't need action (a plain "LGTM").
#[tauri::command]
pub fn dismiss_pr_comments(db: State<DbConnection>, comment_ids: Vec<String>) -> Result<(), String> {
    mark_comments_resolved(&db, &comment_ids, "dismissed")
}

/// Every open PR Fleet is tracking, grouped by project, each with its still-
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
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.name, s.id, s.name,
                    pr.id, pr.session_id, pr.repo_owner, pr.repo_name, pr.number, pr.url, pr.state, pr.last_comment_seen_at, pr.created_at, pr.last_check_error, pr.head_branch,
                    c.id, c.author, c.body, c.comment_created_at, c.path, c.line
             FROM pull_requests pr
             JOIN sessions s ON s.id = pr.session_id
             JOIN projects p ON p.id = s.project_id
             LEFT JOIN pr_review_comments c ON c.pull_request_id = pr.id AND c.resolved_at IS NULL
             WHERE pr.state = 'open'
             ORDER BY p.name, pr.created_at, c.comment_created_at",
        )
        .map_err(|e| e.to_string())?;

    struct Row {
        project_id: String,
        project_name: String,
        session_id: String,
        session_name: String,
        pull_request: PullRequestInfo,
        comment: Option<PrComment>,
    }
    let rows: Vec<Row> = stmt
        .query_map([], |row| {
            let comment_id: Option<String> = row.get(15)?;
            Ok(Row {
                project_id: row.get(0)?,
                project_name: row.get(1)?,
                session_id: row.get(2)?,
                session_name: row.get(3)?,
                pull_request: PullRequestInfo {
                    id: row.get(4)?,
                    session_id: row.get(5)?,
                    repo_owner: row.get(6)?,
                    repo_name: row.get(7)?,
                    number: row.get(8)?,
                    url: row.get(9)?,
                    state: row.get(10)?,
                    last_comment_seen_at: row.get(11)?,
                    created_at: row.get(12)?,
                    last_check_error: row.get(13)?,
                    // Purely an internal freshness cache for check_pr_comments_for's
                    // fast-path decision — not needed for display, and skipped
                    // from serialization anyway.
                    last_seen_updated_at: None,
                    head_branch: row.get(14)?,
                },
                comment: match comment_id {
                    Some(id) => Some(PrComment { id, author: row.get(16)?, body: row.get(17)?, created_at: row.get(18)?, path: row.get(19)?, line: row.get(20)? }),
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
/// finds any open PR of yours on a known project's repo that Fleet isn't
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

/// Finds open PRs authored by the logged-in `gh` user on one project's repo
/// that Fleet doesn't know about yet (created outside Fleet's own "Create
/// PR" button — by hand, or by an agent running `gh` directly inside a
/// turn) and starts tracking them, so the review inbox isn't limited to PRs
/// Fleet itself opened. Anchored to the project's most recently started
/// session purely as an FK/"open session" target — not necessarily the
/// session that actually produced the PR's branch. Silently no-ops for a
/// project with no git remote or no session yet; errors are the caller's to
/// log, not surface, since this runs opportunistically alongside a real
/// check, not as a user-facing action of its own.
fn discover_open_prs_for_project(db: &DbConnection, project_id: &str, project_path: &str) -> Result<(), String> {
    let cwd = Path::new(project_path);
    let (owner, repo) = detect_repo_owner_and_name(cwd)?;
    let repo_full = format!("{owner}/{repo}");
    let output = gh_output(None, &["pr", "list", "--repo", &repo_full, "--state", "open", "--author", "@me", "--json", "number,url,createdAt,headRefName"])?;
    if !output.status.success() {
        return Err(format!("gh pr list failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let list: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let prs = list.as_array().cloned().unwrap_or_default();
    if prs.is_empty() {
        return Ok(());
    }

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    for pr in prs {
        let Some(number) = pr.get("number").and_then(|v| v.as_i64()) else { continue };
        let already_tracked = conn
            .query_row("SELECT 1 FROM pull_requests WHERE repo_owner = ?1 AND repo_name = ?2 AND number = ?3", params![owner, repo, number], |_| Ok(()))
            .optional()
            .map_err(|e| e.to_string())?
            .is_some();
        if already_tracked {
            continue;
        }
        let Some(session_id) = conn
            .query_row("SELECT id FROM sessions WHERE project_id = ?1 ORDER BY started_at DESC LIMIT 1", params![project_id], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())?
        else {
            continue;
        };

        let url = pr.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let created_at = pr.get("createdAt").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let head_branch = pr.get("headRefName").and_then(|v| v.as_str()).map(|s| s.to_string());
        conn.execute(
            "INSERT INTO pull_requests (id, session_id, repo_owner, repo_name, number, url, state, created_at, head_branch) VALUES (?1,?2,?3,?4,?5,?6,'open',?7,?8)",
            params![Uuid::new_v4().to_string(), session_id, owner, repo, number, url, created_at, head_branch],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Runs discovery for every project Fleet knows about — best-effort, a
/// project with no git remote or a `gh` error just gets skipped and logged.
fn discover_open_prs_for_all_projects(db: &DbConnection) {
    let projects: Vec<(String, String)> = {
        let Ok(conn) = db.0.lock() else { return };
        let Ok(mut stmt) = conn.prepare("SELECT id, path FROM projects") else { return };
        let Ok(rows) = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) else { return };
        rows.filter_map(Result::ok).collect()
    };
    for (project_id, project_path) in projects {
        if let Err(e) = discover_open_prs_for_project(db, &project_id, &project_path) {
            log::error!("discover_open_prs_for_project({project_id}): {e}");
        }
    }
}

/// Background poller (see `lib.rs`'s `.setup()`) — checks every open PR,
/// and for any with new comments, emits an event the UI can toast/badge on
/// and fires the opt-in macOS notification. Never sends anything to an
/// agent by itself.
pub fn poll_all_open_prs<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DbConnection>() else { return };
    discover_open_prs_for_all_projects(&db);
    // By PR id, not session_id — a project can have several open PRs
    // anchored to the very same session (see `fetch_pull_request`'s doc
    // comment), and keying this loop by session_id would silently check
    // only one of them, forever.
    let prs: Vec<(String, String)> = {
        let Ok(conn) = db.0.lock() else { return };
        let Ok(mut stmt) = conn.prepare("SELECT id, session_id FROM pull_requests WHERE state = 'open'") else { return };
        let Ok(rows) = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))) else { return };
        rows.filter_map(Result::ok).collect()
    };
    for (pr_id, session_id) in prs {
        match check_pr_comments_for_pr_id(&db, &pr_id) {
            Ok(comments) if !comments.is_empty() => {
                let _ = app.emit("pr-comments-found", serde_json::json!({ "sessionId": session_id, "comments": comments }));
                crate::notifications::notify_if_enabled(
                    app,
                    |s| s.notify_pr_comments,
                    "New PR comments",
                    &format!("{} new comment(s) on a PR Fleet opened", comments.len()),
                );
            }
            Ok(_) => {}
            Err(e) => log::error!("poll_all_open_prs: check failed for PR {pr_id}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_owner_repo_from_url_handles_https_and_ssh_forms() {
        assert_eq!(parse_owner_repo_from_url("https://github.com/killua15/FleetAgent.git"), Some(("killua15".to_string(), "FleetAgent".to_string())));
        assert_eq!(parse_owner_repo_from_url("https://github.com/killua15/FleetAgent"), Some(("killua15".to_string(), "FleetAgent".to_string())));
        assert_eq!(parse_owner_repo_from_url("git@github.com:killua15/FleetAgent.git"), Some(("killua15".to_string(), "FleetAgent".to_string())));
    }

    #[test]
    fn parse_owner_repo_from_url_rejects_non_github_urls() {
        assert_eq!(parse_owner_repo_from_url("https://gitlab.com/killua15/FleetAgent.git"), None);
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
        assert!(body.contains("_Opened by Fleet._"));
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

    #[test]
    fn parse_pr_comments_json_normalizes_comments_and_reviews_and_drops_empty_bodies() {
        let json: Value = serde_json::from_str(
            r#"{
                "comments": [
                    {"author": {"login": "alice"}, "body": "please fix the typo", "createdAt": "2026-01-01T00:00:00Z"}
                ],
                "reviews": [
                    {"author": {"login": "bob"}, "body": "", "submittedAt": "2026-01-01T00:01:00Z", "state": "APPROVED"},
                    {"author": {"login": "carol"}, "body": "one more thing", "submittedAt": "2026-01-01T00:02:00Z", "state": "COMMENTED"}
                ]
            }"#,
        )
        .unwrap();
        let comments = parse_pr_comments_json(&json);
        assert_eq!(comments.len(), 2, "the empty-body approval must be dropped");
        assert_eq!(comments[0].author, "alice");
        assert_eq!(comments[1].author, "carol");
    }

    fn raw(external_id: &str, author: &str, body: &str, created_at: &str) -> RawPrComment {
        RawPrComment { external_id: external_id.to_string(), author: author.to_string(), body: body.to_string(), created_at: created_at.to_string(), path: None, line: None }
    }

    #[test]
    fn parse_inline_comments_json_extracts_file_and_line_and_drops_empty_bodies() {
        let json: Value = serde_json::from_str(
            r#"[
                {"user": {"login": "cmoura2"}, "body": "this lookup can return null", "created_at": "2026-01-01T00:00:00Z", "path": "BridgeRepository.cs", "line": 40},
                {"user": {"login": "cmoura2"}, "body": "", "created_at": "2026-01-01T00:01:00Z", "path": "BridgeRepository.cs", "line": 55},
                {"user": {"login": "cmoura2"}, "body": "consider a null check here too", "created_at": "2026-01-01T00:02:00Z", "path": "BridgeRepository.cs", "line": null, "original_line": 62}
            ]"#,
        )
        .unwrap();
        let comments = parse_inline_comments_json(&json);
        assert_eq!(comments.len(), 2, "the empty-body comment must be dropped");
        assert_eq!(comments[0].path.as_deref(), Some("BridgeRepository.cs"));
        assert_eq!(comments[0].line, Some(40));
        assert_eq!(comments[1].line, Some(62), "must fall back to original_line when line is null");
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
        // fetched, even though its timestamp predates comments Fleet has
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
            "INSERT INTO pull_requests (id, session_id, repo_owner, repo_name, number, url, state, created_at)
             VALUES (?1,?2,'killua15','FleetAgent',1,'https://github.com/killua15/FleetAgent/pull/1','open','now')",
            params![pr_id, session_id],
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
            "INSERT INTO pull_requests (id, session_id, repo_owner, repo_name, number, url, state, created_at)
             VALUES (?1,?2,'killua15','FleetAgent',2,'https://github.com/killua15/FleetAgent/pull/2','open','now')",
            params![pr_id, session_id],
        )
        .unwrap();
    }

    /// Adds a second open PR that reuses an *existing* session's id — the
    /// exact shape `discover_open_prs_for_project` produces when a project
    /// has several open PRs but only one session (every discovered PR
    /// anchors to "the project's most recent session" as its FK target).
    fn insert_open_pr_sharing_session(db: &DbConnection, session_id: &str, pr_id: &str, number: i64, created_at: &str) {
        let conn = db.0.lock().unwrap();
        conn.execute(
            "INSERT INTO pull_requests (id, session_id, repo_owner, repo_name, number, url, state, created_at)
             VALUES (?1,?2,'killua15','FleetAgent',?3,?4,'open',?5)",
            params![pr_id, session_id, number, format!("https://github.com/killua15/FleetAgent/pull/{number}"), created_at],
        )
        .unwrap();
    }

    /// The exact bug reported live: a project with three open PRs
    /// discovered outside Fleet, all sharing the project's one session_id.
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
        let session_id = resolve_session_for_pr_branch(app.handle(), &db, &pr).expect("should resolve without any gh/git calls");
        assert_eq!(session_id, "session-pr1", "the anchor session is already on the right branch — no need to create anything");
    }

    #[test]
    fn resolve_session_for_pr_branch_reuses_an_existing_session_already_on_that_branch() {
        // `setup_db_with_open_pr` gives pr1 number=1, so the deterministic
        // local review branch a previous "Send to agent" would have created
        // *if the PR's real branch name was already taken at the time* is
        // "fleet/pr-1-review" — the reuse lookup must find a session on
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
        let session_id = resolve_session_for_pr_branch(app.handle(), &db, &pr).expect("should resolve without any gh/git calls");
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
            resolve_session_for_pr_branch(app.handle(), &db_state, &pr).expect("should fetch the branch for real and create a linked worktree");
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
        assert!(gitignore.contains(".fleet/"), "the project's .gitignore must cover the worktree folder so it's not untracked clutter");

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
        let new_session_id = resolve_session_for_pr_branch(app.handle(), &db_state, &pr)
            .expect("must succeed even though feat/pr-branch is checked out in another worktree");
        assert_ne!(new_session_id, "session-pr1");

        let worktree_branch: String = {
            let conn = db_state.0.lock().unwrap();
            conn.query_row("SELECT worktree_branch FROM sessions WHERE id = ?1", params![new_session_id], |row| row.get(0)).unwrap()
        };
        assert_eq!(worktree_branch, "fleet/pr-1-review", "the real branch name is taken (exists locally), so it must fall back to Fleet's own name");
        let gitignore = std::fs::read_to_string(work_dir.join(".gitignore")).unwrap_or_default();
        assert!(gitignore.contains(".fleet/"), "the project's .gitignore must cover the worktree folder so it's not untracked clutter");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&other_checkout);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fleet-test-pr-{label}-{}", Uuid::new_v4()));
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
        git_capture(&work_dir, &["config", "user.email", "fleet-test@example.com"]).unwrap();
        git_capture(&work_dir, &["config", "user.name", "Fleet Test"]).unwrap();
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

        let (branch, base) = prepare_branch_for_pr(&work_dir, "My Cool Session").expect("prepare_branch_for_pr should succeed");
        assert_eq!(base, "main");
        assert_eq!(branch, "fleet/my-cool-session");

        let current = git_capture(&work_dir, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(current, "fleet/my-cool-session", "the working dir must actually be on the new branch");

        let remote_branches = git_capture(&remote_dir, &["branch", "--list", "fleet/my-cool-session"]).unwrap();
        assert!(remote_branches.contains("fleet/my-cool-session"), "the branch must be pushed to origin");

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

        let (branch, base) = prepare_branch_for_pr(&work_dir, "Detached Session").expect("prepare_branch_for_pr must handle a detached HEAD, not try to push it literally");
        assert_eq!(base, "main");
        assert_eq!(branch, "fleet/detached-session");

        let remote_branches = git_capture(&remote_dir, &["branch", "--list", "fleet/detached-session"]).unwrap();
        assert!(remote_branches.contains("fleet/detached-session"), "the new branch must be pushed to origin");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn prepare_branch_for_pr_reuses_an_already_checked_out_non_base_branch() {
        let (remote_dir, work_dir) = setup_real_repo_with_remote("existing-branch");
        git_capture(&work_dir, &["checkout", "-b", "already-working-here"]).unwrap();
        std::fs::write(work_dir.join("more.txt"), "more stuff").unwrap();

        let (branch, base) = prepare_branch_for_pr(&work_dir, "irrelevant name").expect("prepare_branch_for_pr should succeed");
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
        let (branch, _base) = prepare_branch_for_pr(&work_dir, "irrelevant").expect("prepare_branch_for_pr should succeed even with nothing to commit");
        assert_eq!(branch, "already-clean");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }
}
