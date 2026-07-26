//! Backend for the git panel screen: a read-only overview of every project's
//! repo plus its active worktrees (linked subsessions), and a handful of
//! basic actions (fetch/pull/push, open in Terminal) per worktree. Reuses the
//! git-remote-detection helpers already built for `pull_requests.rs` rather
//! than re-detecting owner/repo/default-branch from scratch.
//!
//! Every command here either shells out to `git` or spawns `open` — real
//! blocking I/O — so each one is `async` wrapped in `crate::blocking::run_blocking`,
//! matching the fix already applied to `pull_requests.rs` (a non-async
//! `#[tauri::command]` runs inline on the IPC-dispatch thread and would
//! freeze the whole app for as long as the subprocess takes).

use crate::commands::pull_requests::{detect_default_branch, detect_repo_owner_and_name, git_capture, row_to_pull_request, PR_COLUMNS};
use crate::db::DbConnection;
use crate::models::{
    BranchInfo, CommitFileChange, CommitFileDiff, CommitInfo, CommitLogPage, FileStatus, ProjectGitOverview, PullRequestInfo, WorktreeGitStatus,
};
use rusqlite::{params, Connection};
use std::path::Path;
use std::process::Command;
use tauri::{AppHandle, Manager, Runtime};

fn is_git_repo(path: &Path) -> bool {
    path.join(".git").exists()
}

fn current_branch(cwd: &Path) -> Option<String> {
    git_capture(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).ok().filter(|s| !s.is_empty())
}

fn is_dirty(cwd: &Path) -> bool {
    git_capture(cwd, &["status", "--porcelain"]).map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Ahead/behind of `HEAD` vs its tracked upstream if it has one, else vs
/// `fallback_upstream` (typically `origin/<default branch>`) — deliberately
/// doesn't fetch first, so this reflects whatever the local remote-tracking
/// refs already know; the panel's own "Fetch" action is what updates them.
fn ahead_behind_vs_upstream(cwd: &Path, fallback_upstream: &str) -> (i64, i64) {
    let upstream = git_capture(cwd, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback_upstream.to_string());
    if upstream.is_empty() {
        return (0, 0);
    }
    match git_capture(cwd, &["rev-list", "--left-right", "--count", &format!("{upstream}...HEAD")]) {
        Ok(out) => {
            let mut parts = out.split_whitespace();
            let behind = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let ahead = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            (ahead, behind)
        }
        Err(_) => (0, 0),
    }
}

fn fetch_open_prs_for_project(conn: &Connection, project_id: &str) -> Result<Vec<PullRequestInfo>, String> {
    // Straight off `pull_requests.project_id` — a PR no longer has to reach its
    // project through a session, since the ones other people opened have none.
    let mut stmt = conn
        .prepare(&format!("SELECT {PR_COLUMNS} FROM pull_requests WHERE project_id = ?1 AND state = 'open'"))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![project_id], row_to_pull_request).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Matches a worktree to the open PR it's actually reviewing, cheapest/most
/// certain signal first: the PR's own head branch, then the PR's anchor
/// session happening to be this exact worktree's session, then (for a
/// worktree Hangar created via `resolve_session_for_pr_branch`) parsing the PR
/// number back out of its `hangar/pr-<number>-review` branch name — or the
/// pre-rename `fleet/pr-<number>-review`, since worktrees created by the old
/// build still carry that name and must keep linking to their PR.
fn match_linked_pr<'a>(prs: &'a [PullRequestInfo], session_id: &str, branch: Option<&str>) -> Option<&'a PullRequestInfo> {
    if let Some(b) = branch {
        if let Some(pr) = prs.iter().find(|pr| pr.head_branch.as_deref() == Some(b)) {
            return Some(pr);
        }
    }
    if let Some(pr) = prs.iter().find(|pr| pr.session_id.as_deref() == Some(session_id)) {
        return Some(pr);
    }
    let branch = branch?;
    let number: i64 = branch
        .strip_prefix("hangar/pr-")
        .or_else(|| branch.strip_prefix("fleet/pr-"))?
        .strip_suffix("-review")?
        .parse()
        .ok()?;
    prs.iter().find(|pr| pr.number == number)
}

struct WorktreeRow {
    session_id: String,
    session_name: String,
    path: String,
    branch: Option<String>,
    kind: String,
}

struct ProjectRow {
    id: String,
    name: String,
    path: String,
    worktrees: Vec<WorktreeRow>,
    open_prs: Vec<PullRequestInfo>,
}

#[tauri::command]
pub async fn get_git_overview<R: Runtime>(app: AppHandle<R>) -> Result<Vec<ProjectGitOverview>, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();

        // Collect everything the DB knows first, then drop the lock — the
        // rest of this function is a series of `git` subprocess calls that
        // can each take a moment, and no other command should have to wait
        // on the connection mutex while that happens.
        let project_rows: Vec<ProjectRow> = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let mut proj_stmt = conn.prepare("SELECT id, name, path FROM projects ORDER BY created_at ASC").map_err(|e| e.to_string())?;
            let projects: Vec<(String, String, String)> = proj_stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?;
            drop(proj_stmt);

            let mut result = Vec::with_capacity(projects.len());
            for (id, name, path) in projects {
                let open_prs = fetch_open_prs_for_project(&conn, &id)?;
                let mut wt_stmt = conn
                    .prepare(
                        "SELECT id, name, worktree_path, worktree_branch, worktree_kind
                         FROM sessions WHERE project_id = ?1 AND worktree_path IS NOT NULL ORDER BY started_at DESC",
                    )
                    .map_err(|e| e.to_string())?;
                let worktrees = wt_stmt
                    .query_map(params![id], |row| {
                        Ok(WorktreeRow { session_id: row.get(0)?, session_name: row.get(1)?, path: row.get(2)?, branch: row.get(3)?, kind: row.get(4)? })
                    })
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| e.to_string())?;
                result.push(ProjectRow { id, name, path, worktrees, open_prs });
            }
            result
        };

        let mut overviews = Vec::with_capacity(project_rows.len());
        for row in project_rows {
            let root = Path::new(&row.path);
            let (repo_owner, repo_name, default_branch, current_branch_name, ahead, behind, dirty) = if is_git_repo(root) {
                let (owner, name) = detect_repo_owner_and_name(root).map(|(o, n)| (Some(o), Some(n))).unwrap_or((None, None));
                let default_branch = owner.as_ref().map(|_| detect_default_branch(root));
                let fallback_upstream = default_branch.as_deref().map(|b| format!("origin/{b}")).unwrap_or_default();
                let current = current_branch(root);
                let (ahead, behind) = ahead_behind_vs_upstream(root, &fallback_upstream);
                (owner, name, default_branch, current, ahead, behind, is_dirty(root))
            } else {
                (None, None, None, None, 0, 0, false)
            };

            let fallback_upstream = default_branch.as_deref().map(|b| format!("origin/{b}")).unwrap_or_default();
            let mut worktrees = Vec::with_capacity(row.worktrees.len());
            for wt in row.worktrees {
                let wt_root = Path::new(&wt.path);
                if !wt_root.is_dir() {
                    // Stale row (worktree removed on disk without the DB
                    // being updated) — skip it rather than fail the whole
                    // project's overview over one dangling entry.
                    continue;
                }
                let (wt_ahead, wt_behind) = ahead_behind_vs_upstream(wt_root, &fallback_upstream);
                let wt_dirty = is_dirty(wt_root);
                let linked_pr = match_linked_pr(&row.open_prs, &wt.session_id, wt.branch.as_deref()).cloned();
                worktrees.push(WorktreeGitStatus {
                    session_id: wt.session_id,
                    session_name: wt.session_name,
                    branch: wt.branch,
                    kind: wt.kind,
                    path: wt.path,
                    ahead: wt_ahead,
                    behind: wt_behind,
                    dirty: wt_dirty,
                    linked_pr,
                });
            }

            overviews.push(ProjectGitOverview {
                project_id: row.id,
                project_name: row.name,
                project_path: row.path,
                repo_owner,
                repo_name,
                default_branch,
                current_branch: current_branch_name,
                ahead,
                behind,
                dirty,
                worktrees,
            });
        }
        Ok(overviews)
    })
    .await?
}

#[tauri::command]
pub async fn git_fetch(path: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || git_capture(Path::new(&path), &["fetch", "origin"]).map(|_| ())).await?
}

/// `--ff-only` deliberately — this button is meant to be a quick, safe
/// "catch up" action; a real merge/rebase decision belongs in a real git
/// client, not a one-click panel button.
#[tauri::command]
pub async fn git_pull(path: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || git_capture(Path::new(&path), &["pull", "--ff-only"]).map(|_| ())).await?
}

#[tauri::command]
pub async fn git_push(path: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || git_capture(Path::new(&path), &["push"]).map(|_| ())).await?
}

/// "Open in Finder" is handled entirely on the frontend via the opener
/// plugin's `revealItemInDir` — only "open in Terminal" needs a real backend
/// command, since there's no cross-platform plugin API for it and Hangar
/// already assumes macOS elsewhere (see `resolve_binary`/worktree cleanup).
#[tauri::command]
pub async fn open_path_in_terminal(path: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || {
        let status = Command::new("open").args(["-a", "Terminal", &path]).status().map_err(|e| format!("failed to open Terminal: {e}"))?;
        if !status.success() {
            return Err("open -a Terminal failed".to_string());
        }
        Ok(())
    })
    .await?
}

/// `[ahead N, behind M]` / `[ahead N]` / `[behind M]` / `[gone]` / `` (no
/// upstream, or up to date) — the exact text `%(upstream:track)` produces.
fn parse_upstream_track(track: &str) -> (i64, i64) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in track.trim_matches(|c| c == '[' || c == ']').split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            ahead = n.parse().unwrap_or(0);
        }
        if let Some(n) = part.strip_prefix("behind ") {
            behind = n.parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

fn list_branches_impl(cwd: &Path) -> Result<Vec<BranchInfo>, String> {
    let format = "%(refname)\t%(objectname:short)\t%(committerdate:iso-strict)\t%(subject)\t%(upstream:short)\t%(upstream:track)\t%(HEAD)";
    let out = git_capture(cwd, &["for-each-ref", "--sort=-committerdate", &format!("--format={format}"), "refs/heads", "refs/remotes"])?;

    let mut branches = Vec::new();
    for line in out.lines() {
        let mut fields = line.split('\t');
        let refname = fields.next().unwrap_or("");
        let last_commit_hash = fields.next().unwrap_or("").to_string();
        let last_commit_date = fields.next().unwrap_or("").to_string();
        let last_commit_subject = fields.next().unwrap_or("").to_string();
        let upstream = fields.next().unwrap_or("").to_string();
        let track = fields.next().unwrap_or("");
        let head_marker = fields.next().unwrap_or("");

        let (name, is_remote) = if let Some(n) = refname.strip_prefix("refs/heads/") {
            (n.to_string(), false)
        } else if let Some(n) = refname.strip_prefix("refs/remotes/") {
            (n.to_string(), true)
        } else {
            continue;
        };
        // `origin/HEAD` is a symbolic pointer at the remote's default
        // branch, not a real branch — listing it alongside real branches
        // would just be a confusing duplicate entry.
        if name.ends_with("/HEAD") {
            continue;
        }

        let (ahead, behind) = parse_upstream_track(track);
        branches.push(BranchInfo {
            name,
            is_remote,
            is_current: head_marker == "*",
            upstream: if upstream.is_empty() { None } else { Some(upstream) },
            ahead,
            behind,
            last_commit_hash,
            last_commit_subject,
            last_commit_date,
        });
    }
    Ok(branches)
}

/// Every local and remote-tracking branch in one `git` call — ahead/behind
/// comes straight from `%(upstream:track)` rather than a `rev-list` per
/// branch, so this stays cheap even with dozens of branches.
#[tauri::command]
pub async fn list_branches(path: String) -> Result<Vec<BranchInfo>, String> {
    crate::blocking::run_blocking(move || list_branches_impl(Path::new(&path))).await?
}

const COMMIT_FIELD_SEP: char = '\u{1f}';

fn parse_commit_line(line: &str) -> Option<CommitInfo> {
    let mut fields = line.split(COMMIT_FIELD_SEP);
    let hash = fields.next()?.to_string();
    let short_hash = fields.next()?.to_string();
    let parents = fields.next().unwrap_or("").split_whitespace().map(str::to_string).collect();
    let author = fields.next().unwrap_or("").to_string();
    let date = fields.next().unwrap_or("").to_string();
    let subject = fields.next().unwrap_or("").to_string();
    let refs = fields
        .next()
        .unwrap_or("")
        .split(", ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    Some(CommitInfo { hash, short_hash, parents, author, date, subject, refs })
}

fn list_commits_impl(cwd: &Path, branch: Option<&str>, skip: i64, limit: i64) -> Result<CommitLogPage, String> {
    let pretty = format!("--pretty=format:%H{sep}%h{sep}%P{sep}%an{sep}%ad{sep}%s{sep}%D", sep = "%x1f");
    let skip_arg = format!("--skip={skip}");
    let fetch_limit = limit + 1;
    let limit_arg = fetch_limit.to_string();

    let mut args = vec!["log", "--topo-order", "--date=iso-strict", pretty.as_str(), skip_arg.as_str(), "-n", limit_arg.as_str()];
    match branch {
        Some(b) => args.push(b),
        None => args.push("--all"),
    }

    let out = git_capture(cwd, &args)?;
    let mut commits: Vec<CommitInfo> = out.lines().filter_map(parse_commit_line).collect();
    let has_more = commits.len() as i64 > limit;
    commits.truncate(limit.max(0) as usize);
    Ok(CommitLogPage { commits, has_more })
}

/// A page of commit history, newest first — `branch` filters to one ref
/// (`None` means every branch, like GitKraken's default full-graph view).
/// Requests `limit + 1` rows so `has_more` doesn't need a second query.
#[tauri::command]
pub async fn list_commits(path: String, branch: Option<String>, skip: i64, limit: i64) -> Result<CommitLogPage, String> {
    crate::blocking::run_blocking(move || list_commits_impl(Path::new(&path), branch.as_deref(), skip, limit)).await?
}

fn parse_commit_file_changes(name_status: &str) -> Vec<CommitFileChange> {
    let mut changes = Vec::new();
    for line in name_status.lines() {
        let mut fields = line.split('\t');
        let status_code = fields.next().unwrap_or("");
        // Renames/copies (`R100\told\tnew`) carry both paths — the new path
        // (the last field) is the one worth showing, same convention as
        // `group_name_status` in `pull_requests.rs`.
        let path = fields.last().unwrap_or("").trim().to_string();
        if path.is_empty() {
            continue;
        }
        let status = match status_code.chars().next() {
            Some('A') => FileStatus::Added,
            Some('D') => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        changes.push(CommitFileChange { path, status });
    }
    changes
}

fn get_commit_files_impl(cwd: &Path, hash: &str) -> Result<Vec<CommitFileChange>, String> {
    let has_parent = git_capture(cwd, &["rev-parse", "--verify", &format!("{hash}^")]).is_ok();
    let name_status = if has_parent {
        git_capture(cwd, &["diff", "--name-status", &format!("{hash}^"), hash])?
    } else {
        git_capture(cwd, &["show", "--name-status", "--pretty=format:", hash])?
    };
    Ok(parse_commit_file_changes(&name_status))
}

/// Files changed by one commit, diffed against its first parent — a merge
/// commit's *combined* diff isn't shown (that's a genuinely different, much
/// noisier view most git tools hide by default too); the root commit (no
/// parent) falls back to `git show` against the empty tree.
#[tauri::command]
pub async fn get_commit_files(path: String, hash: String) -> Result<Vec<CommitFileChange>, String> {
    crate::blocking::run_blocking(move || get_commit_files_impl(Path::new(&path), &hash)).await?
}

fn get_commit_file_diff_impl(cwd: &Path, hash: &str, file_path: &str) -> CommitFileDiff {
    let old_text = git_capture(cwd, &["show", &format!("{hash}^:{file_path}")]).unwrap_or_default();
    let new_text = git_capture(cwd, &["show", &format!("{hash}:{file_path}")]).unwrap_or_default();
    CommitFileDiff { old_text, new_text }
}

/// One file's before/after content for a commit (again vs. its first
/// parent) — empty string stands in for "didn't exist" (a brand new file, or
/// one this commit deleted), which is exactly what `diffLines` already
/// expects for an add/delete diff.
#[tauri::command]
pub async fn get_commit_file_diff(path: String, hash: String, file_path: String) -> Result<CommitFileDiff, String> {
    crate::blocking::run_blocking(move || get_commit_file_diff_impl(Path::new(&path), &hash, &file_path)).await
}

fn checkout_branch_impl(cwd: &Path, branch: &str, is_remote: bool) -> Result<(), String> {
    if !is_remote {
        return git_capture(cwd, &["checkout", branch]).map(|_| ());
    }
    let local_name = branch.split_once('/').map(|(_, rest)| rest).unwrap_or(branch);
    let local_exists = git_capture(cwd, &["rev-parse", "--verify", local_name]).is_ok();
    if local_exists {
        git_capture(cwd, &["checkout", local_name])?;
    } else {
        git_capture(cwd, &["checkout", "-b", local_name, branch])?;
    }
    Ok(())
}

/// Checking out a remote branch (e.g. `origin/feature-x`) creates Hangar's own
/// local tracking branch the first time (same one-click behavior as
/// GitKraken/GitHub Desktop), then just switches to it on subsequent clicks.
#[tauri::command]
pub async fn checkout_branch(path: String, branch: String, is_remote: bool) -> Result<(), String> {
    crate::blocking::run_blocking(move || checkout_branch_impl(Path::new(&path), &branch, is_remote)).await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-test-gitpanel-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Real bare repo as `origin` + a real working clone with an initial
    /// commit on `main` — same shape as `pull_requests.rs`'s own
    /// `setup_real_repo_with_remote`, so ahead/behind/dirty are computed
    /// against actual git state, not mocked.
    fn setup_repo_with_remote(label: &str) -> (PathBuf, PathBuf) {
        let remote_dir = make_temp_dir(&format!("{label}-remote"));
        git_capture(&remote_dir, &["init", "--bare"]).unwrap();

        let work_dir = make_temp_dir(&format!("{label}-work"));
        git_capture(&work_dir, &["init"]).unwrap();
        git_capture(&work_dir, &["checkout", "-b", "main"]).unwrap();
        git_capture(&work_dir, &["config", "user.email", "hangar-test@example.com"]).unwrap();
        git_capture(&work_dir, &["config", "user.name", "Hangar Test"]).unwrap();
        std::fs::write(work_dir.join("a.txt"), "one").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "initial"]).unwrap();
        git_capture(&work_dir, &["remote", "add", "origin", remote_dir.to_str().unwrap()]).unwrap();
        git_capture(&work_dir, &["push", "-u", "origin", "main"]).unwrap();

        (remote_dir, work_dir)
    }

    #[test]
    fn is_dirty_reflects_uncommitted_changes() {
        let (remote_dir, work_dir) = setup_repo_with_remote("dirty");
        assert!(!is_dirty(&work_dir));
        std::fs::write(work_dir.join("a.txt"), "changed").unwrap();
        assert!(is_dirty(&work_dir));
        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn ahead_behind_reflects_unpushed_and_unpulled_commits() {
        let (remote_dir, work_dir) = setup_repo_with_remote("ahead-behind");

        let (ahead, behind) = ahead_behind_vs_upstream(&work_dir, "origin/main");
        assert_eq!((ahead, behind), (0, 0), "freshly pushed repo must be even with its upstream");

        std::fs::write(work_dir.join("b.txt"), "new").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "local-only commit"]).unwrap();
        let (ahead, behind) = ahead_behind_vs_upstream(&work_dir, "origin/main");
        assert_eq!((ahead, behind), (1, 0), "one unpushed local commit must show up as ahead=1");

        // A second clone pushes a commit the first clone doesn't have yet —
        // fetching (not pulling, to keep the local commit above intact)
        // should surface it as "behind".
        let other_clone = make_temp_dir("ahead-behind-other-clone");
        git_capture(other_clone.parent().unwrap(), &["clone", remote_dir.to_str().unwrap(), other_clone.to_str().unwrap()]).unwrap();
        git_capture(&other_clone, &["config", "user.email", "hangar-test@example.com"]).unwrap();
        git_capture(&other_clone, &["config", "user.name", "Hangar Test"]).unwrap();
        std::fs::write(other_clone.join("c.txt"), "remote-only").unwrap();
        git_capture(&other_clone, &["add", "-A"]).unwrap();
        git_capture(&other_clone, &["commit", "-m", "remote-only commit"]).unwrap();
        git_capture(&other_clone, &["push", "origin", "main"]).unwrap();
        git_capture(&work_dir, &["fetch", "origin"]).unwrap();

        let (ahead, behind) = ahead_behind_vs_upstream(&work_dir, "origin/main");
        assert_eq!((ahead, behind), (1, 1), "diverged: one commit ahead, one behind");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
        let _ = std::fs::remove_dir_all(&other_clone);
    }

    #[test]
    fn match_linked_pr_prefers_head_branch_then_session_then_review_branch_number() {
        let pr_head = sample_pr("pr-head", "other-session", Some("feature/x".to_string()), 10);
        let pr_session = sample_pr("pr-session", "session-2", None, 20);
        let pr_review = sample_pr("pr-review", "other-session-2", None, 30);
        let prs = vec![pr_head, pr_session, pr_review];

        let matched = match_linked_pr(&prs, "session-1", Some("feature/x")).unwrap();
        assert_eq!(matched.id, "pr-head");

        let matched = match_linked_pr(&prs, "session-2", Some("unrelated-branch")).unwrap();
        assert_eq!(matched.id, "pr-session");

        let matched = match_linked_pr(&prs, "session-3", Some("hangar/pr-30-review")).unwrap();
        assert_eq!(matched.id, "pr-review");

        // A worktree created before the Fleet → Hangar rename still carries
        // the old branch name and must keep linking to its PR.
        let matched = match_linked_pr(&prs, "session-3", Some("fleet/pr-30-review")).unwrap();
        assert_eq!(matched.id, "pr-review");

        assert!(match_linked_pr(&prs, "session-4", Some("no-match")).is_none());
    }

    fn sample_pr(id: &str, session_id: &str, head_branch: Option<String>, number: i64) -> PullRequestInfo {
        PullRequestInfo {
            id: id.to_string(),
            project_id: "proj1".to_string(),
            session_id: Some(session_id.to_string()),
            repo_owner: "owner".to_string(),
            repo_name: "repo".to_string(),
            number,
            url: format!("https://github.com/owner/repo/pull/{number}"),
            state: "open".to_string(),
            last_comment_seen_at: None,
            last_check_error: None,
            created_at: "now".to_string(),
            last_seen_updated_at: None,
            head_branch,
            base_branch: None,
            title: None,
            author_login: None,
            relation: "mine".to_string(),
        }
    }

    #[test]
    fn list_branches_reports_current_upstream_and_a_branch_without_one() {
        let (remote_dir, work_dir) = setup_repo_with_remote("list-branches");
        git_capture(&work_dir, &["checkout", "-b", "feature"]).unwrap();
        git_capture(&work_dir, &["checkout", "main"]).unwrap();

        let branches = list_branches_impl(&work_dir).unwrap();

        let main = branches.iter().find(|b| !b.is_remote && b.name == "main").expect("local main");
        assert!(main.is_current, "main is the checked-out branch");
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!((main.ahead, main.behind), (0, 0));

        let feature = branches.iter().find(|b| !b.is_remote && b.name == "feature").expect("local feature");
        assert!(!feature.is_current);
        assert!(feature.upstream.is_none(), "feature was never pushed, so it has no upstream");

        let remote_main = branches.iter().find(|b| b.is_remote && b.name == "origin/main").expect("remote-tracking origin/main");
        assert!(!remote_main.last_commit_hash.is_empty());

        assert!(branches.iter().all(|b| b.name != "origin/HEAD"), "the symbolic origin/HEAD ref must be filtered out");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn list_commits_captures_both_parents_of_a_real_merge_commit() {
        let (remote_dir, work_dir) = setup_repo_with_remote("list-commits");

        git_capture(&work_dir, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(work_dir.join("feature.txt"), "feature work").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "feature commit"]).unwrap();

        git_capture(&work_dir, &["checkout", "main"]).unwrap();
        std::fs::write(work_dir.join("main.txt"), "main work").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "main commit"]).unwrap();

        git_capture(&work_dir, &["merge", "--no-ff", "-m", "merge feature into main", "feature"]).unwrap();

        let page = list_commits_impl(&work_dir, None, 0, 50).unwrap();
        assert!(!page.has_more);
        assert_eq!(page.commits.len(), 4, "initial + main commit + feature commit + merge commit");

        let merge_commit = page.commits.iter().find(|c| c.subject == "merge feature into main").expect("merge commit present");
        assert_eq!(merge_commit.parents.len(), 2, "a real merge commit must carry both parent hashes");

        let feature_commit = page.commits.iter().find(|c| c.subject == "feature commit").expect("feature commit present");
        assert_eq!(feature_commit.parents.len(), 1);

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn list_commits_paginates_with_skip_and_limit() {
        let (remote_dir, work_dir) = setup_repo_with_remote("list-commits-paginate");
        for i in 0..5 {
            std::fs::write(work_dir.join(format!("f{i}.txt")), format!("{i}")).unwrap();
            git_capture(&work_dir, &["add", "-A"]).unwrap();
            git_capture(&work_dir, &["commit", "-m", &format!("commit {i}")]).unwrap();
        }

        let first_page = list_commits_impl(&work_dir, None, 0, 2).unwrap();
        assert_eq!(first_page.commits.len(), 2);
        assert!(first_page.has_more);
        assert_eq!(first_page.commits[0].subject, "commit 4", "newest first");

        let second_page = list_commits_impl(&work_dir, None, 2, 2).unwrap();
        assert_eq!(second_page.commits.len(), 2);
        assert_eq!(second_page.commits[0].subject, "commit 2");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn get_commit_files_and_diff_reflect_add_modify_delete() {
        let (remote_dir, work_dir) = setup_repo_with_remote("commit-files");
        std::fs::write(work_dir.join("to-delete.txt"), "will be removed").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "add to-delete.txt"]).unwrap();

        std::fs::write(work_dir.join("a.txt"), "one\nchanged").unwrap();
        std::fs::write(work_dir.join("new.txt"), "brand new").unwrap();
        std::fs::remove_file(work_dir.join("to-delete.txt")).unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "add/modify/delete"]).unwrap();

        let hash = git_capture(&work_dir, &["rev-parse", "HEAD"]).unwrap();
        let changes = get_commit_files_impl(&work_dir, &hash).unwrap();

        let a_status = changes.iter().find(|c| c.path == "a.txt").unwrap().status;
        assert_eq!(a_status, FileStatus::Modified);
        let new_status = changes.iter().find(|c| c.path == "new.txt").unwrap().status;
        assert_eq!(new_status, FileStatus::Added);
        let deleted_status = changes.iter().find(|c| c.path == "to-delete.txt").unwrap().status;
        assert_eq!(deleted_status, FileStatus::Deleted);

        let diff = get_commit_file_diff_impl(&work_dir, &hash, "a.txt");
        assert_eq!(diff.old_text, "one");
        assert_eq!(diff.new_text, "one\nchanged");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn checkout_branch_switches_locally_and_creates_a_tracking_branch_for_remotes() {
        let (remote_dir, work_dir) = setup_repo_with_remote("checkout");
        git_capture(&work_dir, &["checkout", "-b", "feature"]).unwrap();
        git_capture(&work_dir, &["checkout", "main"]).unwrap();

        checkout_branch_impl(&work_dir, "feature", false).unwrap();
        assert_eq!(current_branch(&work_dir).as_deref(), Some("feature"));

        // Simulate a branch that only exists on the remote (a second clone
        // pushes it, the first clone only knows about it after a fetch).
        let other_clone = make_temp_dir("checkout-other-clone");
        git_capture(other_clone.parent().unwrap(), &["clone", remote_dir.to_str().unwrap(), other_clone.to_str().unwrap()]).unwrap();
        git_capture(&other_clone, &["config", "user.email", "hangar-test@example.com"]).unwrap();
        git_capture(&other_clone, &["config", "user.name", "Hangar Test"]).unwrap();
        git_capture(&other_clone, &["checkout", "-b", "remote-only"]).unwrap();
        std::fs::write(other_clone.join("remote-only.txt"), "hi").unwrap();
        git_capture(&other_clone, &["add", "-A"]).unwrap();
        git_capture(&other_clone, &["commit", "-m", "remote-only commit"]).unwrap();
        git_capture(&other_clone, &["push", "origin", "remote-only"]).unwrap();
        git_capture(&work_dir, &["fetch", "origin"]).unwrap();

        checkout_branch_impl(&work_dir, "origin/remote-only", true).unwrap();
        assert_eq!(current_branch(&work_dir).as_deref(), Some("remote-only"), "checking out a remote branch must create a same-named local branch");
        assert!(work_dir.join("remote-only.txt").exists());

        // A second checkout of the same remote branch must reuse the local
        // branch rather than failing on "branch already exists".
        checkout_branch_impl(&work_dir, "main", false).unwrap();
        checkout_branch_impl(&work_dir, "origin/remote-only", true).unwrap();
        assert_eq!(current_branch(&work_dir).as_deref(), Some("remote-only"));

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
        let _ = std::fs::remove_dir_all(&other_clone);
    }
}
