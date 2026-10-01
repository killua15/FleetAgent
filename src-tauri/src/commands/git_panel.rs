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

use crate::commands::pull_requests::{detect_default_branch, git_capture, row_to_pull_request, PR_COLUMNS};
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
fn match_linked_pr<'a>(prs: &'a [PullRequestInfo], session_id: Option<&str>, branch: Option<&str>) -> Option<&'a PullRequestInfo> {
    if let Some(b) = branch {
        if let Some(pr) = prs.iter().find(|pr| pr.head_branch.as_deref() == Some(b)) {
            return Some(pr);
        }
    }
    if let Some(pr) = session_id.and_then(|id| prs.iter().find(|pr| pr.session_id.as_deref() == Some(id))) {
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
    account_id: Option<String>,
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

/// Every worktree of one project: the ones sessions own, plus the ones only
/// git knows about.
///
/// Sessions come first and keep their order. A session whose folder is gone is
/// kept as `missing` rather than dropped — dropping it is what hid stale rows
/// until now, with nothing to clean them up from. Orphans come from `git
/// worktree list` minus the main worktree and minus anything a session owns.
fn collect_worktrees(
    project_root: &Path,
    is_repo: bool,
    sessions: Vec<WorktreeRow>,
    open_prs: &[PullRequestInfo],
    fallback_upstream: &str,
) -> Vec<WorktreeGitStatus> {
    use crate::commands::sessions::{list_worktree_entries, same_path, LEGACY_WORKTREES_DIR, WORKTREES_DIR};

    let hangar_dirs = [project_root.join(WORKTREES_DIR).join("worktrees"), project_root.join(LEGACY_WORKTREES_DIR).join("worktrees")];
    let is_external = |path: &str| !hangar_dirs.iter().any(|dir| Path::new(path).starts_with(dir));
    let status = |session_id: Option<String>, account_id: Option<String>, name: String, path: String, branch: Option<String>, kind: String, missing: bool| {
        let (ahead, behind, dirty) = if missing {
            (0, 0, false)
        } else {
            let root = Path::new(&path);
            let (ahead, behind) = ahead_behind_vs_upstream(root, fallback_upstream);
            (ahead, behind, is_dirty(root))
        };
        let linked_pr = match_linked_pr(open_prs, session_id.as_deref(), branch.as_deref()).cloned();
        let external = is_external(&path);
        WorktreeGitStatus { session_id, session_name: name, account_id, branch, kind, path, ahead, behind, dirty, linked_pr, missing, external }
    };

    let entries = if is_repo { list_worktree_entries(project_root).unwrap_or_default() } else { Vec::new() };
    let mut out = Vec::with_capacity(sessions.len() + entries.len());
    for wt in &sessions {
        let missing = !Path::new(&wt.path).is_dir();
        out.push(status(Some(wt.session_id.clone()), wt.account_id.clone(), wt.session_name.clone(), wt.path.clone(), wt.branch.clone(), wt.kind.clone(), missing));
    }
    for entry in entries.into_iter().skip(1) {
        if sessions.iter().any(|wt| same_path(&wt.path, &entry.path)) {
            continue;
        }
        let name = Path::new(&entry.path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| entry.path.clone());
        let missing = entry.prunable || !Path::new(&entry.path).is_dir();
        out.push(status(None, None, name, entry.path, entry.branch, "linked".to_string(), missing));
    }
    out
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
                        "SELECT id, name, worktree_path, worktree_branch, worktree_kind, account_id
                         FROM sessions WHERE project_id = ?1 AND worktree_path IS NOT NULL ORDER BY started_at DESC",
                    )
                    .map_err(|e| e.to_string())?;
                let worktrees = wt_stmt
                    .query_map(params![id], |row| {
                        Ok(WorktreeRow { session_id: row.get(0)?, session_name: row.get(1)?, path: row.get(2)?, branch: row.get(3)?, kind: row.get(4)?, account_id: row.get(5)? })
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
            let is_repo = is_git_repo(root);
            // Whole `RepoRef`, not just the owner/name half of it: the forge and
            // the instance are what the provider picker groups these projects by,
            // and `detect_repo` has already worked both out from `origin`.
            let repo = is_repo.then(|| crate::forges::detect_repo(root).ok()).flatten();
            let (default_branch, current_branch_name, ahead, behind, dirty) = if is_repo {
                let default_branch = repo.as_ref().map(|_| detect_default_branch(root));
                let fallback_upstream = default_branch.as_deref().map(|b| format!("origin/{b}")).unwrap_or_default();
                let current = current_branch(root);
                let (ahead, behind) = ahead_behind_vs_upstream(root, &fallback_upstream);
                (default_branch, current, ahead, behind, is_dirty(root))
            } else {
                (None, None, 0, 0, false)
            };
            let (repo_owner, repo_name, forge, host) = match repo {
                Some(r) => (Some(r.namespace), Some(r.name), Some(r.forge.as_str().to_string()), Some(r.host)),
                None => (None, None, None, None),
            };

            let fallback_upstream = default_branch.as_deref().map(|b| format!("origin/{b}")).unwrap_or_default();
            let worktrees = collect_worktrees(root, is_repo, row.worktrees, &row.open_prs, &fallback_upstream);

            overviews.push(ProjectGitOverview {
                project_id: row.id,
                project_name: row.name,
                project_path: row.path,
                repo_owner,
                repo_name,
                forge,
                host,
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

/// "Open in Finder" / "Show in Explorer" is handled entirely on the frontend
/// via the opener plugin's `revealItemInDir` — only "open in Terminal" needs a
/// real backend command, since there's no cross-platform plugin API for it and
/// every OS names its terminal something different.
#[tauri::command]
pub async fn open_path_in_terminal(path: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || open_terminal_at(&path)).await?
}

/// Where JetBrains Rider lives on this Mac, if it's installed — so the review
/// panel only offers "Open in Rider" when it can do it. Task worktrees live in
/// `.hangar/worktrees/…`, outside whatever folder Rider has open, which is why
/// Rider never showed their changes.
fn rider_app() -> Option<std::path::PathBuf> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let home = crate::app_data::home_dir().unwrap_or_default();
    ["/Applications/Rider.app".to_string(), format!("{home}/Applications/Rider.app")]
        .into_iter()
        .map(std::path::PathBuf::from)
        .find(|p| p.is_dir())
}

#[tauri::command]
pub fn is_rider_installed() -> bool {
    rider_app().is_some()
}

/// Opens a worktree (or a single file in it) in Rider.
#[tauri::command]
pub async fn open_in_rider(path: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || {
        let app = rider_app().ok_or("Rider isn't installed in /Applications")?;
        let status = Command::new("open").arg("-a").arg(&app).arg(&path).status().map_err(|e| format!("failed to open Rider: {e}"))?;
        if !status.success() {
            return Err(format!("Rider could not open {path}"));
        }
        Ok(())
    })
    .await?
}

#[cfg(target_os = "macos")]
fn open_terminal_at(path: &str) -> Result<(), String> {
    let status = Command::new("open").args(["-a", "Terminal", path]).status().map_err(|e| format!("failed to open Terminal: {e}"))?;
    if !status.success() {
        return Err("open -a Terminal failed".to_string());
    }
    Ok(())
}

/// Windows Terminal first, then a plain console window.
///
/// `wt.exe` is the modern one and takes a working directory directly, but it
/// only exists if the user installed it (it ships with Windows 11 and not with
/// every 10). `cmd /C start` is the fallback that is always there — `start`
/// needs its first quoted argument to be the *window title*, hence the empty
/// `""`, or it would swallow the shell path as the title and open nothing.
#[cfg(target_os = "windows")]
fn open_terminal_at(path: &str) -> Result<(), String> {
    if let Ok(status) = Command::new("wt.exe").args(["-d", path]).status() {
        if status.success() {
            return Ok(());
        }
    }
    let status = Command::new("cmd")
        .args(["/C", "start", "", "powershell.exe", "-NoExit", "-Command", &format!("Set-Location -LiteralPath '{}'", path.replace('\'', "''"))])
        .status()
        .map_err(|e| format!("failed to open a terminal: {e}"))?;
    if !status.success() {
        return Err("could not open a terminal window".to_string());
    }
    Ok(())
}

/// `x-terminal-emulator` is Debian's alternatives-system name for "whatever
/// terminal this machine has", and the rest are the common ones by name. Linux
/// isn't a target Hangar builds for today, but leaving this arm out would make
/// the module fail to compile there rather than fail to find a terminal.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn open_terminal_at(path: &str) -> Result<(), String> {
    for terminal in ["x-terminal-emulator", "gnome-terminal", "konsole", "xterm"] {
        if let Ok(status) = Command::new(terminal).arg("--working-directory").arg(path).status() {
            if status.success() {
                return Ok(());
            }
        }
    }
    Err("no terminal emulator found".to_string())
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

/// What a worktree currently holds, split the way you actually act on it.
///
/// Two lists, deliberately not merged. **Uncommitted** answers "what is the
/// agent doing to my repo right now" and changes second by second;
/// **committed** answers "what did this task produce, is it ready to PR" and
/// is the unit you review. Showing them as one list would mean a file that's
/// both — edited now *and* changed earlier in the branch — silently loses one
/// of its two states.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeChanges {
    /// What the branch is measured against: its upstream when it has one, else
    /// the project's default branch. The same one `ahead_behind_vs_upstream`
    /// uses, so the card's ▲2 and this diff can never disagree.
    pub base: String,
    pub uncommitted: Vec<ChangedFile>,
    pub committed: Vec<ChangedFile>,
}

#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub status: FileStatus,
    pub added: i64,
    pub removed: i64,
    /// True when the file is staged — shown, but not acted on: this view is
    /// read-only, and a stage/discard button here is where one mis-click
    /// destroys an agent's work with no undo.
    pub staged: bool,
}

/// `git status --porcelain` plus `--numstat`, joined.
///
/// Two calls rather than one because `status` says *what* changed and
/// `numstat` says *how much*, and no single porcelain command gives both.
fn uncommitted_changes(cwd: &Path) -> Vec<ChangedFile> {
    let status = git_capture(cwd, &["status", "--porcelain"]).unwrap_or_default();
    let unstaged = numstat_map(&git_capture(cwd, &["diff", "--numstat"]).unwrap_or_default());
    let staged = numstat_map(&git_capture(cwd, &["diff", "--cached", "--numstat"]).unwrap_or_default());

    let mut files = Vec::new();
    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        let (codes, rest) = line.split_at(2);
        // A rename reads `R  old -> new`; the new name is the one to show, same
        // convention as everywhere else here.
        let path = rest.trim().rsplit(" -> ").next().unwrap_or("").trim_matches('"').to_string();
        if path.is_empty() {
            continue;
        }
        let index = codes.chars().next().unwrap_or(' ');
        let worktree = codes.chars().nth(1).unwrap_or(' ');
        let status = match (index, worktree) {
            ('?', _) => FileStatus::Untracked,
            ('A', _) => FileStatus::Added,
            (_, 'D') | ('D', _) => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        let (added, removed) = staged
            .get(&path)
            .or_else(|| unstaged.get(&path))
            .copied()
            // An untracked file has no diff to count: every line of it is new,
            // and saying nothing would make a 400-line addition look like a
            // rounding error next to a 2-line edit.
            .unwrap_or_else(|| match status {
                FileStatus::Untracked => (count_lines(&cwd.join(&path)), 0),
                _ => (0, 0),
            });
        files.push(ChangedFile { path, status, added, removed, staged: index != ' ' && index != '?' });
    }
    files
}

fn count_lines(path: &Path) -> i64 {
    // Capped: a checked-in 200 MB fixture shouldn't be read into memory to put
    // a number next to its name.
    match std::fs::metadata(path).map(|m| m.len()) {
        Ok(size) if size < 2_000_000 => std::fs::read_to_string(path).map(|text| text.lines().count() as i64).unwrap_or(0),
        _ => 0,
    }
}

fn numstat_map(raw: &str) -> std::collections::BTreeMap<String, (i64, i64)> {
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let added = fields.next()?;
            let removed = fields.next()?;
            let path = fields.last()?.trim().to_string();
            // Binary files come back as `-\t-\tpath`, which parses to zero and
            // is the honest answer: there are no lines to count.
            Some((path, (added.parse().unwrap_or(0), removed.parse().unwrap_or(0))))
        })
        .collect()
}

/// What this branch added on top of its base, `base...HEAD` — three dots, so
/// commits that landed on the base *after* this branch started don't show up
/// as if this branch had removed them.
fn committed_changes(cwd: &Path, base: &str) -> Vec<ChangedFile> {
    let range = format!("{base}...HEAD");
    let numstat = numstat_map(&git_capture(cwd, &["diff", "--numstat", &range]).unwrap_or_default());
    let name_status = git_capture(cwd, &["diff", "--name-status", &range]).unwrap_or_default();
    parse_commit_file_changes(&name_status)
        .into_iter()
        .map(|change| {
            let (added, removed) = numstat.get(&change.path).copied().unwrap_or((0, 0));
            ChangedFile { path: change.path, status: change.status, added, removed, staged: false }
        })
        .collect()
}

/// What "this branch's changes" are measured against, in order of how much it
/// knows: the base the user picked, the one the worktree was cut from, the
/// upstream when it's a *different* branch, and the default branch.
///
/// The upstream used to come first, unconditionally — and a task branch pushed
/// to its own `origin/<same name>` was then compared against itself, so the
/// review showed only unpushed commits instead of the feature.
fn resolve_base(cwd: &Path, requested: Option<String>, stored: Option<String>) -> String {
    if let Some(base) = requested.or(stored).filter(|b| !b.trim().is_empty()) {
        return base;
    }
    let current = current_branch(cwd);
    if let Ok(upstream) = git_capture(cwd, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]) {
        let same_branch = upstream.split_once('/').map(|(_, name)| name) == current.as_deref();
        if !upstream.is_empty() && !same_branch {
            return upstream;
        }
    }
    let default = detect_default_branch(cwd);
    // A worktree sitting *on* the default branch has nothing to compare against
    // itself; its upstream is the honest base.
    if current.as_deref() == Some(default.as_str()) {
        return format!("origin/{default}");
    }
    default
}

/// The base Hangar recorded when it cut the worktree at `cwd`, if it did.
fn stored_base<R: Runtime>(app: &AppHandle<R>, cwd: &Path) -> Option<String> {
    let db = app.try_state::<DbConnection>()?;
    let conn = db.0.lock().ok()?;
    let mut stmt = conn.prepare("SELECT worktree_path, worktree_base FROM sessions WHERE worktree_base IS NOT NULL").ok()?;
    let rows: Vec<(String, String)> = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).ok()?.filter_map(Result::ok).collect();
    let cwd = cwd.to_string_lossy();
    rows.into_iter().find(|(path, _)| crate::commands::sessions::same_path(path, &cwd)).map(|(_, base)| base)
}

#[tauri::command]
pub async fn get_worktree_changes<R: Runtime>(app: AppHandle<R>, path: String, base: Option<String>) -> Result<WorktreeChanges, String> {
    crate::blocking::run_blocking(move || {
        let cwd = Path::new(&path);
        if !is_git_repo(cwd) && git_capture(cwd, &["rev-parse", "--git-dir"]).is_err() {
            return Err(format!("{path} isn't a git worktree"));
        }
        let base = resolve_base(cwd, base, stored_base(&app, cwd));
        Ok(WorktreeChanges { uncommitted: uncommitted_changes(cwd), committed: committed_changes(cwd, &base), base })
    })
    .await?
}

/// One file's before/after, for either list.
///
/// `committed` picks which question is being asked: the branch's version
/// against the base, or the working copy against the last commit. The same
/// file can legitimately appear in both with different content.
#[tauri::command]
pub async fn get_worktree_file_diff<R: Runtime>(app: AppHandle<R>, path: String, file_path: String, base: Option<String>, committed: bool) -> Result<CommitFileDiff, String> {
    crate::blocking::run_blocking(move || {
        let cwd = Path::new(&path);
        let base = resolve_base(cwd, base, stored_base(&app, cwd));
        if committed {
            // Against where the branch started, not the base's tip: the file
            // list is `base...HEAD`, and a diff against the tip would show
            // whatever landed on the base since as this branch undoing it.
            let from = git_capture(cwd, &["merge-base", &base, "HEAD"]).unwrap_or(base);
            return CommitFileDiff {
                old_text: git_capture(cwd, &["show", &format!("{from}:{file_path}")]).unwrap_or_default(),
                new_text: git_capture(cwd, &["show", &format!("HEAD:{file_path}")]).unwrap_or_default(),
            };
        }
        CommitFileDiff {
            // Empty for a file that didn't exist at HEAD — an untracked file —
            // which is what the differ already reads as "all of this is new".
            old_text: git_capture(cwd, &["show", &format!("HEAD:{file_path}")]).unwrap_or_default(),
            // Straight off disk: this is the whole point of the uncommitted
            // view, and `git show` can't see what was never committed.
            new_text: std::fs::read_to_string(cwd.join(&file_path)).unwrap_or_default(),
        }
    })
    .await
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

    /// What the panel lists for one project: a session's worktree, a session
    /// whose folder is gone (kept, marked missing — it used to vanish from
    /// the list), and one git knows about that no session owns, made outside
    /// `.hangar/worktrees` so it reads as another tool's.
    #[test]
    fn worktrees_include_orphans_and_sessions_whose_folder_is_gone() {
        let repo = make_temp_dir("collect");
        git_capture(&repo, &["init"]).unwrap();
        std::fs::write(repo.join("README.md"), "hi").unwrap();
        git_capture(&repo, &["add", "-A"]).unwrap();
        git_capture(&repo, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-m", "init"]).unwrap();

        let owned = repo.join(".hangar/worktrees/owned");
        std::fs::create_dir_all(owned.parent().unwrap()).unwrap();
        git_capture(&repo, &["worktree", "add", &owned.to_string_lossy(), "-b", "owned"]).unwrap();
        let outside = make_temp_dir("collect-outside").join("elsewhere");
        git_capture(&repo, &["worktree", "add", &outside.to_string_lossy(), "-b", "elsewhere"]).unwrap();

        let row = |id: &str, path: &Path| WorktreeRow {
            session_id: id.to_string(),
            session_name: id.to_string(),
            account_id: None,
            path: path.to_string_lossy().into_owned(),
            branch: None,
            kind: "linked".to_string(),
        };
        let sessions = vec![row("s-owned", &owned), row("s-gone", &repo.join(".hangar/worktrees/gone"))];
        let list = collect_worktrees(&repo, true, sessions, &[], "");

        assert_eq!(list.len(), 3, "{list:?}");
        assert_eq!(list[0].session_id.as_deref(), Some("s-owned"));
        assert!(!list[0].missing && !list[0].external);
        assert_eq!(list[1].session_id.as_deref(), Some("s-gone"));
        assert!(list[1].missing, "a session whose folder is gone is shown, not dropped");
        assert_eq!(list[2].session_id, None, "a worktree no session owns is an orphan");
        assert_eq!(list[2].session_name, "elsewhere");
        assert_eq!(list[2].branch.as_deref(), Some("elsewhere"));
        assert!(list[2].external);

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(outside.parent().unwrap());
    }

    /// The two lists have to stay apart. A file edited *now* and a file this
    /// branch changed *earlier* are different questions with different
    /// answers, and one file can be in both at once — which is exactly the
    /// case a single merged list would silently collapse.
    #[test]
    fn uncommitted_and_committed_changes_are_reported_separately() {
        let (remote_dir, work_dir) = setup_repo_with_remote("changes");
        git_capture(&work_dir, &["checkout", "-b", "feature"]).unwrap();

        // Committed on the branch: one edit, one new file.
        std::fs::write(work_dir.join("README.md"), "hello\nfrom the branch\n").unwrap();
        std::fs::write(work_dir.join("added.txt"), "one\ntwo\n").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "branch work"]).unwrap();

        // And then, uncommitted: the same file again, plus an untracked one.
        std::fs::write(work_dir.join("README.md"), "hello\nfrom the branch\nand a live edit\n").unwrap();
        std::fs::write(work_dir.join("scratch.txt"), "a\nb\nc\n").unwrap();

        let changes = tauri::async_runtime::block_on(get_worktree_changes(
            tauri::test::mock_app().handle().clone(),
            work_dir.to_string_lossy().to_string(),
            Some("main".to_string()),
        ))
        .unwrap();

        let uncommitted: Vec<&str> = changes.uncommitted.iter().map(|f| f.path.as_str()).collect();
        assert!(uncommitted.contains(&"README.md"), "the live edit belongs here: {uncommitted:?}");
        assert!(uncommitted.contains(&"scratch.txt"), "an untracked file is a change: {uncommitted:?}");

        let committed: Vec<&str> = changes.committed.iter().map(|f| f.path.as_str()).collect();
        assert!(committed.contains(&"added.txt"), "committed branch work: {committed:?}");
        assert!(committed.contains(&"README.md"), "a file can be in both lists at once: {committed:?}");

        let scratch = changes.uncommitted.iter().find(|f| f.path == "scratch.txt").unwrap();
        assert_eq!(scratch.status, FileStatus::Untracked);
        assert_eq!(scratch.added, 3, "an untracked file counts all its lines — otherwise a 400-line addition reads as nothing");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    /// `base...HEAD`, not `base..HEAD`. With two dots, every commit that
    /// landed on the base after this branch started shows up as if the branch
    /// had deleted it — which on a busy `dev` is most of the diff.
    #[test]
    fn commits_that_landed_on_the_base_later_are_not_counted_as_this_branch_s_work() {
        let (remote_dir, work_dir) = setup_repo_with_remote("three-dot");
        git_capture(&work_dir, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(work_dir.join("mine.txt"), "mine\n").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "my work"]).unwrap();

        // Meanwhile, someone else lands something on main.
        git_capture(&work_dir, &["checkout", "main"]).unwrap();
        std::fs::write(work_dir.join("theirs.txt"), "theirs\n").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "their work"]).unwrap();
        git_capture(&work_dir, &["checkout", "feature"]).unwrap();

        let changes = tauri::async_runtime::block_on(get_worktree_changes(
            tauri::test::mock_app().handle().clone(),
            work_dir.to_string_lossy().to_string(),
            Some("main".to_string()),
        ))
        .unwrap();

        let committed: Vec<&str> = changes.committed.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(committed, vec!["mine.txt"], "only this branch's own work: {committed:?}");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    /// A task branch pushed to its own `origin/<same name>` used to be compared
    /// against that upstream — itself — so the review listed only unpushed
    /// commits. Its own upstream is skipped; the base it was cut from wins.
    #[test]
    fn a_branch_is_not_compared_against_its_own_upstream() {
        let (remote_dir, work_dir) = setup_repo_with_remote("own-upstream");
        git_capture(&work_dir, &["checkout", "-b", "dev"]).unwrap();
        git_capture(&work_dir, &["push", "-u", "origin", "dev"]).unwrap();
        git_capture(&work_dir, &["checkout", "-b", "task-1"]).unwrap();
        std::fs::write(work_dir.join("task.txt"), "done\n").unwrap();
        git_capture(&work_dir, &["add", "-A"]).unwrap();
        git_capture(&work_dir, &["commit", "-m", "task work"]).unwrap();
        git_capture(&work_dir, &["push", "-u", "origin", "task-1"]).unwrap();

        assert_ne!(resolve_base(&work_dir, None, None), "origin/task-1", "never itself");
        assert_eq!(resolve_base(&work_dir, None, Some("dev".to_string())), "dev", "the stored base wins over guessing");
        assert_eq!(resolve_base(&work_dir, Some("main".to_string()), Some("dev".to_string())), "main", "an explicit pick wins over both");

        let committed: Vec<String> = committed_changes(&work_dir, "dev").into_iter().map(|f| f.path).collect();
        assert_eq!(committed, vec!["task.txt".to_string()], "the feature, even though it's all pushed");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    /// A committed file's diff starts where the branch started. Against the
    /// base's tip, a change that landed on the base afterwards would read as
    /// this branch undoing it.
    #[test]
    fn a_committed_diff_starts_where_the_branch_started() {
        let (remote_dir, work_dir) = setup_repo_with_remote("merge-base-diff");
        git_capture(&work_dir, &["checkout", "-b", "feature"]).unwrap();
        std::fs::write(work_dir.join("a.txt"), "one\nmine\n").unwrap();
        git_capture(&work_dir, &["commit", "-am", "mine"]).unwrap();
        git_capture(&work_dir, &["checkout", "main"]).unwrap();
        std::fs::write(work_dir.join("a.txt"), "one\ntheirs on main\n").unwrap();
        git_capture(&work_dir, &["commit", "-am", "theirs"]).unwrap();
        git_capture(&work_dir, &["checkout", "feature"]).unwrap();

        let diff = tauri::async_runtime::block_on(get_worktree_file_diff(
            tauri::test::mock_app().handle().clone(),
            work_dir.to_string_lossy().to_string(),
            "a.txt".to_string(),
            Some("main".to_string()),
            true,
        ))
        .unwrap();
        assert_eq!(diff.old_text.trim(), "one", "the old side is where the branch started");
        assert!(!diff.old_text.contains("theirs"), "main's later change is not this branch's to undo");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    /// The uncommitted view reads the working copy off disk, because `git
    /// show` cannot see what was never committed — the exact content the user
    /// is asking about.
    #[test]
    fn an_uncommitted_diff_reads_the_file_from_disk() {
        let (remote_dir, work_dir) = setup_repo_with_remote("disk-diff");
        std::fs::write(work_dir.join("README.md"), "hello\nedited but not committed\n").unwrap();

        let diff = tauri::async_runtime::block_on(get_worktree_file_diff(
            tauri::test::mock_app().handle().clone(),
            work_dir.to_string_lossy().to_string(),
            "README.md".to_string(),
            Some("main".to_string()),
            false,
        ))
        .unwrap();
        assert!(diff.new_text.contains("edited but not committed"));
        assert!(!diff.old_text.contains("edited but not committed"), "the old side is HEAD, not the disk");

        let _ = std::fs::remove_dir_all(&remote_dir);
        let _ = std::fs::remove_dir_all(&work_dir);
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

        let matched = match_linked_pr(&prs, Some("session-1"), Some("feature/x")).unwrap();
        assert_eq!(matched.id, "pr-head");

        let matched = match_linked_pr(&prs, Some("session-2"), Some("unrelated-branch")).unwrap();
        assert_eq!(matched.id, "pr-session");

        let matched = match_linked_pr(&prs, Some("session-3"), Some("hangar/pr-30-review")).unwrap();
        assert_eq!(matched.id, "pr-review");

        // A worktree created before the Fleet → Hangar rename still carries
        // the old branch name and must keep linking to its PR.
        let matched = match_linked_pr(&prs, Some("session-3"), Some("fleet/pr-30-review")).unwrap();
        assert_eq!(matched.id, "pr-review");

        assert!(match_linked_pr(&prs, Some("session-4"), Some("no-match")).is_none());
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
            forge: "github".to_string(),
            host: "github.com".to_string(),
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
