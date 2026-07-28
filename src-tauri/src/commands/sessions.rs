use crate::credential_vault;
use crate::db::DbConnection;
use crate::models::{AccountUsageSummary, FileEdit, FileStatus, FileTreeNode, NewSubsession, ResumableConversation, Session, SessionBlock, SessionView, TurnUsage};
use crate::process_manager;
use crate::providers::{self, claude, Mode};
use crate::turn_runner::{self, TurnRequest};
use chrono::Utc;
use rusqlite::{params, Connection};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::Command;
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

const SESSION_VIEW_COLUMNS: &str = "s.id, s.name, s.project_id, p.name, s.parent_session_id, s.worktree_branch, s.worktree_kind, s.account_id, a.name, a.provider, a.scope, s.model, s.effort, s.mode, s.status, s.started_at";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartSessionInput {
    pub project_id: String,
    pub account_id: String,
    pub name: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
}

pub(crate) fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        name: row.get("name")?,
        project_id: row.get("project_id")?,
        node_id: row.get("node_id")?,
        parent_session_id: row.get("parent_session_id")?,
        worktree_path: row.get("worktree_path")?,
        worktree_branch: row.get("worktree_branch")?,
        worktree_kind: row.get("worktree_kind")?,
        account_id: row.get("account_id")?,
        model: row.get("model")?,
        effort: row.get("effort")?,
        mode: row.get("mode")?,
        status: row.get("status")?,
        pid: row.get("pid")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
    })
}

fn row_to_session_view(row: &rusqlite::Row) -> rusqlite::Result<SessionView> {
    Ok(SessionView {
        id: row.get(0)?,
        name: row.get(1)?,
        project_id: row.get(2)?,
        project_name: row.get(3)?,
        parent_session_id: row.get(4)?,
        worktree_branch: row.get(5)?,
        worktree_kind: row.get(6)?,
        account_id: row.get(7)?,
        account_name: row.get(8)?,
        provider: row.get(9)?,
        scope: row.get(10)?,
        model: row.get(11)?,
        effort: row.get(12)?,
        mode: row.get(13)?,
        status: row.get(14)?,
        started_at: row.get(15)?,
    })
}

/// Creates the session row only — no process is spawned until the first
/// composer message, mirroring the AgentPanel mockup (open a session, then
/// type). Each subsequent message is its own headless `claude -p` turn.
#[tauri::command]
pub fn start_session(db: State<DbConnection>, input: StartSessionInput) -> Result<Session, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    conn.query_row(
        "SELECT id FROM accounts WHERE id = ?1",
        params![input.account_id],
        |row| row.get::<_, String>(0),
    )
    .map_err(|e| format!("account lookup failed: {e}"))?;
    conn.query_row(
        "SELECT id FROM projects WHERE id = ?1",
        params![input.project_id],
        |row| row.get::<_, String>(0),
    )
    .map_err(|e| format!("project lookup failed: {e}"))?;

    let session_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let name = if input.name.trim().is_empty() { "untitled session".to_string() } else { input.name.trim().to_string() };
    conn.execute(
        "INSERT INTO sessions (id, name, project_id, node_id, account_id, model, effort, mode, status, pid, provider_session_id, started_at, ended_at)
         VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, 'waiting', NULL, NULL, ?8, NULL)",
        params![session_id, name, input.project_id, input.account_id, input.model, input.effort, input.mode, now],
    )
    .map_err(|e| e.to_string())?;

    conn.query_row("SELECT * FROM sessions WHERE id = ?1", params![session_id], row_to_session)
        .map_err(|e| e.to_string())
}

// ".hangar" must always be skipped, not just as a size optimization: the
// worktree destination for a "copy" subsession lives at
// "<project_path>/.hangar/worktrees/<slug>", nested *inside* the very source
// tree being copied. Without this exclusion, copy_project_dir would recurse
// into its own destination directory as it walks the source, copying the
// copy into itself indefinitely (surfaces as "File name too long").
// ".fleet" is the pre-rename location and stays on the list for as long as
// any project can still contain worktrees created by the old build.
pub(crate) const COPY_SKIP_DIRS: &[&str] = &["node_modules", "dist", "target", ".next", "__pycache__", "venv", ".venv", "build", ".git", ".hangar", ".fleet"];

/// Directory, inside a managed project, that holds Hangar's worktrees.
pub(crate) const WORKTREES_DIR: &str = ".hangar";

/// Where the pre-rename build put them. Still read (never written) so
/// worktrees created before the rename keep being recognized.
pub(crate) const LEGACY_WORKTREES_DIR: &str = ".fleet";
const ENV_FILE_CANDIDATES: &[&str] = &[".env", ".env.local", ".env.development", ".env.production"];

/// Lowercase, ASCII-alphanumeric-only, dash-separated — used for both the
/// worktree's folder name and its branch name so both stay filesystem- and
/// git-ref-safe regardless of what the user typed as the subsession name.
pub(crate) fn slugify(input: &str) -> String {
    let mut slug: String = input
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        format!("subsession-{}", &Uuid::new_v4().to_string()[..8])
    } else {
        slug
    }
}

/// Picks `.hangar/worktrees/<slug>`, disambiguating with a short suffix if a
/// worktree with that name already exists (e.g. two subsessions both named
/// "fix-bug").
///
/// A slug already taken under the pre-rename `.fleet/worktrees` counts as
/// taken too: the branch name is derived from this same slug, and git refuses
/// to create a branch that already exists — so ignoring the legacy directory
/// would turn every name reused from before the rename into a failed
/// `git worktree add` instead of a silently disambiguated one.
pub(crate) fn unique_worktree_dir(project_path: &str, slug: &str) -> PathBuf {
    let project = Path::new(project_path);
    let base = project.join(WORKTREES_DIR).join("worktrees");
    let candidate = base.join(slug);
    let legacy_candidate = project.join(LEGACY_WORKTREES_DIR).join("worktrees").join(slug);
    if !candidate.exists() && !legacy_candidate.exists() {
        return candidate;
    }
    let suffix = &Uuid::new_v4().to_string()[..6];
    base.join(format!("{slug}-{suffix}"))
}

fn project_is_git_repo(project_path: &str) -> bool {
    Command::new(providers::resolve_binary("git"))
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(project_path)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new(providers::resolve_binary("git"))
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("failed to run git {}: {e}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

/// Real `git worktree` off the project's existing repo, branched from the
/// project's local HEAD (not the remote default) so the subsession starts
/// from whatever's already checked out, including uncommitted... no —
/// worktrees only carry committed history, same as real `git worktree add`.
/// Cuts the worktree. `base` is the branch (or ref) the new one starts from;
/// `None` means whatever the project has checked out.
///
/// Naming the base matters more than it looks: branching off whatever happened
/// to be checked out means a worktree cut while you were on a half-finished
/// feature silently inherits it, and the agent's diff then contains work that
/// isn't its own.
/// Checks an **existing** branch out into a worktree.
///
/// The failure worth handling is git refusing because that branch is already
/// checked out somewhere — most likely the user's own main clone, precisely
/// because they were working on it. Git's own message names the other path, so
/// it's kept and prefixed rather than replaced with something vaguer.
fn attach_linked_worktree(project_path: &str, worktree_dir: &Path, branch: &str) -> Result<(), String> {
    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    run_git(Path::new(project_path), &["worktree", "add", &worktree_dir.to_string_lossy(), branch])
        .map_err(|e| format!("could not check out `{branch}` in a worktree: {e}"))
}

fn create_linked_worktree(project_path: &str, worktree_dir: &Path, branch: &str, base: Option<&str>) -> Result<(), String> {
    if let Some(parent) = worktree_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let dir = worktree_dir.to_string_lossy().to_string();
    let mut args = vec!["worktree", "add", &dir, "-b", branch];
    if let Some(base) = base {
        args.push(base);
    }
    run_git(Path::new(project_path), &args)
}

/// Recursively copies a project directory into an isolated folder, skipping
/// heavy/generated directories we don't want to duplicate (no `.gitignore`
/// to consult here since the whole point of this path is "no git repo").
fn copy_project_dir(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if COPY_SKIP_DIRS.iter().any(|skip| name.to_string_lossy() == *skip) {
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        let file_type = entry.file_type().map_err(|e| e.to_string())?;
        if file_type.is_dir() {
            copy_project_dir(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            std::fs::copy(&src_path, &dst_path).map_err(|e| e.to_string())?;
        }
        // Symlinks are skipped deliberately — copying them naively risks
        // following a link outside the project entirely.
    }
    Ok(())
}

/// For projects with no git repo: copies the folder, then gives the copy its
/// own fresh history so the agent can still use git commands inside it —
/// there's no existing repo to link a worktree to.
fn init_fresh_git_repo(dir: &Path) {
    let _ = run_git(dir, &["init"]);
    let _ = run_git(dir, &["add", "-A"]);
    // Best-effort: commit can fail (e.g. no git user configured); an empty
    // repo still lets the agent run git commands, just without history yet.
    let _ = run_git(dir, &["commit", "-m", "Initial snapshot"]);
    let _ = run_git(dir, &["branch", "-M", "main"]);
}

fn copy_env_files(project_path: &str, worktree_dir: &Path) {
    for name in ENV_FILE_CANDIDATES {
        let src = Path::new(project_path).join(name);
        if src.is_file() {
            let _ = std::fs::copy(&src, worktree_dir.join(name));
        }
    }
}

/// Appends `.hangar/` to the *project's* `.gitignore` (creating the file if
/// missing) so worktree folders don't show up as untracked clutter in the
/// user's own `git status` — best-effort, never blocks subsession creation.
/// `pub(crate)` so `resolve_session_for_pr_branch` (its own worktree lands
/// at the same `.hangar/worktrees/...` location) can call it too.
///
/// A project already ignoring the pre-rename `.fleet/` still gets the new
/// entry added — that's where new worktrees go now, so matching on the legacy
/// line would leave them untracked and visible in `git status`.
pub(crate) fn ensure_worktree_dir_ignored(project_path: &str) {
    let gitignore_path = Path::new(project_path).join(".gitignore");
    let existing = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == ".hangar/" || l.trim() == ".hangar") {
        return;
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("\n# Hangar subsession worktrees\n.hangar/\n");
    let _ = std::fs::write(&gitignore_path, updated);
}

fn run_setup_command(worktree_dir: &Path, command: &str) -> Result<String, String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(worktree_dir)
        .output()
        .map_err(|e| format!("failed to run setup command: {e}"))?;
    let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() {
        return Err(format!("setup command exited with {:?}: {}", output.status.code(), combined.trim()));
    }
    Ok(combined.trim().to_string())
}

/// Branches an existing session into a subsession that works in its own
/// isolated directory — a real `git worktree` when the project is a git
/// repo, or a plain copy with a fresh `git init` when it isn't (see
/// `create_linked_worktree`/`init_fresh_git_repo`). `send_message` already
/// prefers `worktree_path` over the project's own path as the turn's cwd
/// once this session has one set.
/// Everything the worktree machinery needs, decoupled from *who asked*.
///
/// Exists because a worktree stopped being an agent-only idea: the terminals
/// create them too, and they have no parent session to hang off. The two callers
/// differ only in where the project comes from and what a session row should say
/// about model/effort/mode — everything below (the real `git worktree add`, the
/// copy fallback for a non-repo, the `.env` copy, the gitignore safeguard, the
/// setup command) is identical and must stay identical.
pub(crate) struct WorktreeRequest {
    pub project_id: String,
    pub project_path: String,
    pub default_setup_command: Option<String>,
    /// The session this branches off, when there is one. `None` for a worktree
    /// created straight from a project — which is what a terminal does.
    pub parent_session_id: Option<String>,
    pub name: String,
    pub account_id: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub setup_command: Option<String>,
    pub branch: WorktreeBranch,
}

/// Which branch a worktree ends up on.
///
/// Three cases and not one, because the three callers want genuinely different
/// things and collapsing them is what made the task flow impossible:
///
/// - An agent subsession or a terminal wants a **namespaced** branch
///   (`subsession/…`, `hangar/…`): deterministic, and it can't collide with a
///   branch the user actually cares about.
/// - Working a task wants the branch **named exactly** as the task — `T2`, not
///   `hangar/t2` — because that name is what the PR, the CI and the rest of the
///   team see. A namespace there would be Hangar putting itself in the middle
///   of someone else's convention.
/// - And sometimes the branch **already exists** and the point is to check it
///   out, not to make another one.
#[derive(Debug, Clone)]
pub enum WorktreeBranch {
    /// Check out a branch that already exists in this repo.
    Existing(String),
    /// Create `<prefix>/<slug(name)>`, off `base` or off whatever is checked
    /// out when `base` is `None`.
    Namespaced { name: String, prefix: &'static str, base: Option<String> },
    /// Create a branch with exactly this name. For task branches, where the
    /// name is not ours to decorate.
    Exact { name: String, base: Option<String> },
}

impl WorktreeBranch {
    /// The branch's final name — what `git branch` will report inside the
    /// worktree.
    pub fn resolved_name(&self) -> String {
        match self {
            WorktreeBranch::Existing(name) => name.clone(),
            WorktreeBranch::Namespaced { name, prefix, .. } => format!("{prefix}/{}", slugify(name)),
            WorktreeBranch::Exact { name, .. } => name.clone(),
        }
    }
}

/// What a terminal-created worktree records for the columns a terminal never
/// reads: the CLI picks its own model and permission mode once it's running, so
/// storing a guess here would be a second source of truth nobody consults.
pub(crate) const TERMINAL_WORKTREE_EFFORT: &str = "high";
pub(crate) const TERMINAL_WORKTREE_MODE: &str = "ask_edits";

#[tauri::command]
pub async fn create_subsession<R: tauri::Runtime>(app: AppHandle<R>, input: NewSubsession) -> Result<Session, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (project_id, project_path, default_setup_command): (String, String, Option<String>) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT id FROM accounts WHERE id = ?1",
                params![input.account_id],
                |row| row.get::<_, String>(0),
            )
            .map_err(|e| format!("account lookup failed: {e}"))?;
            let project_id: String = conn
                .query_row(
                    "SELECT project_id FROM sessions WHERE id = ?1",
                    params![input.parent_session_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("parent session lookup failed: {e}"))?;
            let (project_path, default_setup_command): (String, Option<String>) = conn
                .query_row(
                    "SELECT path, default_setup_command FROM projects WHERE id = ?1",
                    params![project_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(|e| format!("project lookup failed: {e}"))?;
            (project_id, project_path, default_setup_command)
        };

        let session_id = create_worktree_session(
            &app,
            &db,
            WorktreeRequest {
                project_id,
                project_path,
                default_setup_command,
                parent_session_id: Some(input.parent_session_id),
                name: input.name.clone(),
                account_id: input.account_id,
                model: input.model,
                effort: input.effort,
                mode: input.mode,
                setup_command: input.setup_command,
                branch: WorktreeBranch::Namespaced { name: input.name.clone(), prefix: "subsession", base: None },
            },
        )?;

        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT * FROM sessions WHERE id = ?1", params![session_id], row_to_session)
            .map_err(|e| e.to_string())
    })
    .await?
}

/// Cuts a worktree for a project with no session behind it, and returns the
/// session id that owns it — what a terminal opens against.
///
/// The session row here is a container for the working directory, not an agent:
/// nothing will run a headless turn on it. That's why model/effort/mode aren't
/// asked for (see `TERMINAL_WORKTREE_*`).
#[tauri::command]
pub async fn create_worktree_for_project<R: tauri::Runtime>(
    app: AppHandle<R>,
    project_id: String,
    name: String,
    account_id: String,
    setup_command: Option<String>,
    base_branch: Option<String>,
) -> Result<Session, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (project_path, default_setup_command, model) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT id FROM accounts WHERE id = ?1", params![account_id], |row| row.get::<_, String>(0))
                .map_err(|e| format!("account lookup failed: {e}"))?;
            let (project_path, default_setup_command): (String, Option<String>) = conn
                .query_row("SELECT path, default_setup_command FROM projects WHERE id = ?1", params![project_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|e| format!("project lookup failed: {e}"))?;
            // Inherited from the project's newest session when there is one,
            // purely so the row looks like its neighbours; empty otherwise.
            let model: String = conn
                .query_row(
                    "SELECT model FROM sessions WHERE project_id = ?1 ORDER BY started_at DESC LIMIT 1",
                    params![project_id],
                    |row| row.get(0),
                )
                .unwrap_or_default();
            (project_path, default_setup_command, model)
        };

        // A worktree is a git object. Without a repo there is nothing to branch
        // from, no base to choose, and no diff to review at the end — so this
        // says so instead of quietly copying the folder into something that
        // looks like a worktree and behaves like a photocopy.
        if !project_is_git_repo(&project_path) {
            return Err(format!(
                "{project_path} isn't a git repository, so Hangar can't cut a worktree for it. Run `git init` there (and make one commit) first."
            ));
        }

        let session_id = create_worktree_session(
            &app,
            &db,
            WorktreeRequest {
                project_id,
                project_path,
                default_setup_command,
                parent_session_id: None,
                name: name.clone(),
                account_id,
                model,
                effort: TERMINAL_WORKTREE_EFFORT.to_string(),
                mode: TERMINAL_WORKTREE_MODE.to_string(),
                setup_command,
                branch: WorktreeBranch::Namespaced { name: name.clone(), prefix: "hangar", base: base_branch },
            },
        )?;

        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT * FROM sessions WHERE id = ?1", params![session_id], row_to_session)
            .map_err(|e| e.to_string())
    })
    .await?
}

/// Cuts a worktree on a branch chosen by the caller — the task flow's entry
/// point, where the branch is the task's own name rather than something Hangar
/// invents.
///
/// Shares everything below with the other two callers; only the branch decision
/// differs, which is the whole reason `WorktreeBranch` exists.
pub(crate) async fn create_worktree_on_branch<R: tauri::Runtime>(
    app: AppHandle<R>,
    project_id: String,
    branch: WorktreeBranch,
    account_id: String,
) -> Result<Session, String> {
    let name = branch.resolved_name();
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (project_path, default_setup_command, model) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT id FROM accounts WHERE id = ?1", params![account_id], |row| row.get::<_, String>(0))
                .map_err(|e| format!("account lookup failed: {e}"))?;
            let (project_path, default_setup_command): (String, Option<String>) = conn
                .query_row("SELECT path, default_setup_command FROM projects WHERE id = ?1", params![project_id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(|e| format!("project lookup failed: {e}"))?;
            let model: String = conn
                .query_row("SELECT model FROM sessions WHERE project_id = ?1 ORDER BY started_at DESC LIMIT 1", params![project_id], |row| row.get(0))
                .unwrap_or_default();
            (project_path, default_setup_command, model)
        };

        if !project_is_git_repo(&project_path) {
            return Err(format!("{project_path} isn't a git repository, so Hangar can't cut a worktree for it."));
        }

        let session_id = create_worktree_session(
            &app,
            &db,
            WorktreeRequest {
                project_id,
                project_path,
                default_setup_command,
                parent_session_id: None,
                name,
                account_id,
                model,
                effort: TERMINAL_WORKTREE_EFFORT.to_string(),
                mode: TERMINAL_WORKTREE_MODE.to_string(),
                setup_command: None,
                branch,
            },
        )?;

        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT * FROM sessions WHERE id = ?1", params![session_id], row_to_session)
            .map_err(|e| e.to_string())
    })
    .await?
}

/// The worktree creation itself — unchanged behaviour, just no longer tied to
/// having a parent session.
pub(crate) fn create_worktree_session<R: tauri::Runtime>(app: &AppHandle<R>, db: &DbConnection, input: WorktreeRequest) -> Result<String, String> {
    {
        let project_path = &input.project_path;
        let slug = slugify(&input.name);
        let worktree_dir = unique_worktree_dir(project_path, &slug);
        let is_git = project_is_git_repo(project_path);
        let branch = input.branch.resolved_name();

        let worktree_kind = if is_git {
            match &input.branch {
                // Attaching, not creating: the branch is already there and the
                // point is to work on it.
                WorktreeBranch::Existing(name) => attach_linked_worktree(project_path, &worktree_dir, name)?,
                WorktreeBranch::Namespaced { base, .. } | WorktreeBranch::Exact { base, .. } => {
                    create_linked_worktree(project_path, &worktree_dir, &branch, base.as_deref())?
                }
            }
            "linked"
        } else {
            copy_project_dir(Path::new(project_path), &worktree_dir)?;
            init_fresh_git_repo(&worktree_dir);
            "copy"
        };
        let worktree_branch: Option<String> = if is_git { Some(branch) } else { None };

        copy_env_files(project_path, &worktree_dir);
        ensure_worktree_dir_ignored(project_path);
        // Written into the worktree's own config, not just passed as
        // environment: the agent runs `git commit` itself in here, in a process
        // whose environment Hangar no longer controls once the shell is live.
        crate::git_identity::apply_identity_to_worktree(app, &input.project_id, &worktree_dir);

        let effective_setup_command = input.setup_command.clone().or(input.default_setup_command.clone());

        let session_id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let name = if input.name.trim().is_empty() { "untitled worktree".to_string() } else { input.name.trim().to_string() };
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO sessions (id, name, project_id, node_id, parent_session_id, worktree_path, worktree_branch, worktree_kind, account_id, model, effort, mode, status, pid, provider_session_id, started_at, ended_at)
                 VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'waiting', NULL, NULL, ?12, NULL)",
                params![
                    session_id,
                    name,
                    input.project_id,
                    input.parent_session_id,
                    worktree_dir.to_string_lossy().to_string(),
                    worktree_branch,
                    worktree_kind,
                    input.account_id,
                    input.model,
                    input.effort,
                    input.mode,
                    now,
                ],
            )
            .map_err(|e| e.to_string())?;
            if let Some(cmd) = &input.setup_command {
                let _ = conn.execute("UPDATE projects SET default_setup_command = ?2 WHERE id = ?1", params![input.project_id, cmd]);
            }
        }

        let kind_label = if is_git {
            format!("linked worktree on branch `{}`", worktree_branch.as_deref().unwrap_or(""))
        } else {
            "copied folder with a fresh git init".to_string()
        };
        process_manager::emit_block(app, &session_id, "agent", &format!("↳ worktree created — {kind_label} at {}", worktree_dir.display()));

        if let Some(cmd) = &effective_setup_command {
            process_manager::emit_block(app, &session_id, "agent", &format!("⋯ running setup command `{cmd}`…"));
            match run_setup_command(&worktree_dir, cmd) {
                Ok(out) => {
                    let text = if out.is_empty() { format!("setup command `{cmd}` finished") } else { format!("setup command `{cmd}`:\n{out}") };
                    process_manager::emit_block(app, &session_id, "agent", &text);
                }
                Err(e) => {
                    process_manager::emit_block(app, &session_id, "agent", &format!("setup command `{cmd}` failed: {e}"));
                }
            }
        }

        Ok(session_id)
    }
}

#[tauri::command]
pub fn list_project_sessions(db: State<DbConnection>, project_id: String) -> Result<Vec<Session>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM sessions WHERE project_id = ?1 ORDER BY started_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], row_to_session)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_session_view(db: State<DbConnection>, session_id: String) -> Result<SessionView, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        &format!(
            "SELECT {SESSION_VIEW_COLUMNS}
             FROM sessions s
             JOIN projects p ON p.id = s.project_id
             JOIN accounts a ON a.id = s.account_id
             WHERE s.id = ?1"
        ),
        params![session_id],
        row_to_session_view,
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_session_settings(
    db: State<DbConnection>,
    session_id: String,
    model: String,
    effort: String,
    mode: String,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sessions SET model = ?2, effort = ?3, mode = ?4 WHERE id = ?1",
        params![session_id, model, effort, mode],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Joined view used by MainView's panel grid + sidebar — every session
/// across every project/account, with the display fields already resolved
/// so the frontend doesn't have to stitch together three separate lists.
#[tauri::command]
pub fn list_workspace_sessions(db: State<DbConnection>) -> Result<Vec<SessionView>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {SESSION_VIEW_COLUMNS}
             FROM sessions s
             JOIN projects p ON p.id = s.project_id
             JOIN accounts a ON a.id = s.account_id
             ORDER BY s.started_at ASC"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_to_session_view).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Runs one turn against an existing session: fetches its account/project/
/// mode, resolves credentials + config dir isolation, and hands off to the
/// provider's turn runner (only Claude Code is wired so far).
#[tauri::command]
pub fn send_message(app: AppHandle, db: State<DbConnection>, session_id: String, text: String) -> Result<(), String> {
    // Failover before anything else reads the session row: if this session's
    // account is rate-limited and the project allows rotating, this repoints
    // it at a usable account (and clears the now-meaningless `--resume` id)
    // so everything below picks up the new account naturally.
    let switch = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::rotation::rotate_session_if_needed(&conn, &session_id)?
    };
    if let Some(switch) = &switch {
        process_manager::emit_block(&app, &session_id, "agent", &crate::rotation::switch_notice(switch));
    }

    let (provider, billing, config_dir_path, account_id, cwd, name, model, effort, mode_str, resume) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let (account_id, project_id, name, model, effort, mode_str, resume, worktree_path): (
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT account_id, project_id, name, model, effort, mode, provider_session_id, worktree_path FROM sessions WHERE id = ?1",
                params![session_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .map_err(|e| format!("session lookup failed: {e}"))?;
        let (provider, billing, config_dir_path): (String, String, String) = conn
            .query_row(
                "SELECT provider, billing, config_dir_path FROM accounts WHERE id = ?1",
                params![account_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("account lookup failed: {e}"))?;
        let project_path: String = conn
            .query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
            .map_err(|e| format!("project lookup failed: {e}"))?;
        // A subsession runs its turns inside its own isolated worktree/copy
        // instead of the shared project path, if it has one.
        let cwd = worktree_path.unwrap_or(project_path);
        (provider, billing, config_dir_path, account_id, cwd, name, model, effort, mode_str, resume)
    };

    let api_key = if billing == "key" {
        Some(credential_vault::get(&account_id)?)
    } else {
        None
    };
    let env = claude::env_for_account(&config_dir_path, api_key.as_deref());

    turn_runner::spawn_turn(TurnRequest {
        app,
        session_id,
        account_id,
        provider,
        cwd,
        env,
        mode: Mode::from_str(&mode_str),
        model,
        effort,
        name,
        resume,
        prompt: text,
    })
}

/// Aborts whatever this session is currently doing and parks it as `done`.
/// `kill_turn` also flags the turn as user-cancelled, so the turn's own
/// background thread skips its normal tail instead of racing this write back
/// to `waiting` a moment later (see `turn_runner::CancelledTurns`).
#[tauri::command]
pub fn stop_session(app: AppHandle, db: State<DbConnection>, session_id: String) -> Result<(), String> {
    let _ = process_manager::kill(&app, &session_id);
    let _ = turn_runner::kill_turn(&app, &session_id);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sessions SET status = 'done', ended_at = ?2 WHERE id = ?1",
        params![session_id, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) struct WorktreeCleanupRow {
    pub worktree_path: String,
    pub worktree_branch: Option<String>,
    pub worktree_kind: String,
    pub project_path: String,
}

/// Physically removes each subsession's isolated directory (and its branch,
/// for a linked worktree) — best-effort, never fails the caller's DB delete.
/// `ON DELETE CASCADE` on `parent_session_id`/`account_id` only removes DB
/// rows; it doesn't know about the filesystem, so callers gather the rows
/// that are *about to be* cascaded and pass them here first.
pub(crate) fn cleanup_worktree_rows(rows: &[WorktreeCleanupRow]) {
    for row in rows {
        let path = Path::new(&row.worktree_path);
        if !path.exists() {
            continue;
        }
        if row.worktree_kind == "linked" {
            let project_dir = Path::new(&row.project_path);
            if let Err(e) = run_git(project_dir, &["worktree", "remove", "--force", &row.worktree_path]) {
                log::error!("cleanup_worktree_rows: git worktree remove failed for {}: {e}", row.worktree_path);
            }
            if let Some(branch) = &row.worktree_branch {
                if let Err(e) = run_git(project_dir, &["branch", "-D", branch]) {
                    log::error!("cleanup_worktree_rows: git branch -D failed for {branch}: {e}");
                }
            }
        } else if let Err(e) = std::fs::remove_dir_all(path) {
            log::error!("cleanup_worktree_rows: remove_dir_all failed for {}: {e}", row.worktree_path);
        }
    }
}

fn worktree_cleanup_rows_for_session_subtree(conn: &rusqlite::Connection, session_id: &str) -> Vec<WorktreeCleanupRow> {
    let mut stmt = match conn.prepare(
        "WITH RECURSIVE descendants(id) AS (
           SELECT id FROM sessions WHERE id = ?1
           UNION ALL
           SELECT s.id FROM sessions s JOIN descendants d ON s.parent_session_id = d.id
         )
         SELECT s.worktree_path, s.worktree_branch, s.worktree_kind, p.path
         FROM sessions s
         JOIN projects p ON p.id = s.project_id
         WHERE s.id IN (SELECT id FROM descendants) AND s.worktree_path IS NOT NULL",
    ) {
        Ok(stmt) => stmt,
        Err(e) => {
            log::error!("worktree_cleanup_rows_for_session_subtree: query failed: {e}");
            return Vec::new();
        }
    };
    let rows = stmt.query_map(params![session_id], |row| {
        Ok(WorktreeCleanupRow {
            worktree_path: row.get(0)?,
            worktree_branch: row.get(1)?,
            worktree_kind: row.get(2)?,
            project_path: row.get(3)?,
        })
    });
    match rows {
        Ok(rows) => rows.filter_map(Result::ok).collect(),
        Err(e) => {
            log::error!("worktree_cleanup_rows_for_session_subtree: query_map failed: {e}");
            Vec::new()
        }
    }
}

/// Removes a session entirely (not just marks it done) — cascades to its
/// blocks, checkpoints, and any subsessions (and their subsessions, ...).
/// Kills any tracked live process first, best-effort, and physically cleans
/// up every worktree/copy in the subtree before the DB row cascade removes
/// the rows that reference them.
#[tauri::command]
pub async fn delete_session<R: tauri::Runtime>(app: AppHandle<R>, session_id: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let _ = process_manager::kill(&app, &session_id);
        let _ = turn_runner::kill_turn(&app, &session_id);
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let cleanup_rows = worktree_cleanup_rows_for_session_subtree(&conn, &session_id);
        cleanup_worktree_rows(&cleanup_rows);
        conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await?
}

#[tauri::command]
pub fn list_session_blocks(db: State<DbConnection>, session_id: String) -> Result<Vec<SessionBlock>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM session_blocks WHERE session_id = ?1 ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![session_id], |row| {
            Ok(SessionBlock {
                id: row.get("id")?,
                session_id: row.get("session_id")?,
                kind: row.get("kind")?,
                text: row.get("text")?,
                created_at: row.get("created_at")?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Real before/after content of every Edit/Write/MultiEdit this session made
/// — used by the diff panel, filtered client-side down to one file's history.
#[tauri::command]
pub fn list_file_edits(db: State<DbConnection>, session_id: String) -> Result<Vec<FileEdit>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, session_id, block_id, file_path, tool_name, old_text, new_text, created_at FROM file_edits WHERE session_id = ?1 ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![session_id], |row| {
            Ok(FileEdit {
                id: row.get(0)?,
                session_id: row.get(1)?,
                block_id: row.get(2)?,
                file_path: row.get(3)?,
                tool_name: row.get(4)?,
                old_text: row.get(5)?,
                new_text: row.get(6)?,
                created_at: row.get(7)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_turn_usage(db: State<DbConnection>, session_id: String) -> Result<Vec<TurnUsage>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT id, session_id, block_id, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, cost_usd, created_at
             FROM turn_usage WHERE session_id = ?1 ORDER BY created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![session_id], |row| {
            Ok(TurnUsage {
                id: row.get(0)?,
                session_id: row.get(1)?,
                block_id: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                cache_creation_tokens: row.get(6)?,
                cost_usd: row.get(7)?,
                created_at: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Hangar's own recorded spend for an account since `since_iso` — not a live
/// pull of the provider's real quota (Hangar has no API access to that), just
/// a sum over the turns Hangar itself has captured.
#[tauri::command]
pub fn get_account_usage_summary(db: State<DbConnection>, account_id: String, since_iso: String) -> Result<AccountUsageSummary, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT COALESCE(SUM(tu.input_tokens), 0), COALESCE(SUM(tu.output_tokens), 0), COALESCE(SUM(tu.cost_usd), 0.0)
         FROM turn_usage tu
         JOIN sessions s ON s.id = tu.session_id
         WHERE s.account_id = ?1 AND tu.created_at >= ?2",
        params![account_id, since_iso],
        |row| {
            Ok(AccountUsageSummary {
                input_tokens: row.get(0)?,
                output_tokens: row.get(1)?,
                cost_usd: row.get(2)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

/// Parses `git status --porcelain` into a relative-path → status map,
/// following the VS Code convention: untracked/added → green, modified →
/// amber, deleted → red. Best-effort — an empty map (no coloring at all) is
/// a safe fallback if git isn't available or the command fails.
fn git_status_map(cwd: &Path) -> HashMap<String, FileStatus> {
    let mut map = HashMap::new();
    let Ok(output) = Command::new(providers::resolve_binary("git")).args(["status", "--porcelain"]).current_dir(cwd).output() else {
        return map;
    };
    if !output.status.success() {
        return map;
    }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if line.len() < 4 {
            continue;
        }
        let code = &line[0..2];
        let raw_path = line[3..].trim();
        // Renames show as "R  old -> new" — the tree only knows about the new path.
        let path = raw_path.split(" -> ").last().unwrap_or(raw_path).trim_matches('"').to_string();
        let status = if code.contains('D') {
            FileStatus::Deleted
        } else if code == "??" {
            FileStatus::Untracked
        } else if code.contains('A') {
            FileStatus::Added
        } else {
            FileStatus::Modified
        };
        map.insert(path, status);
    }
    map
}

/// Same idea as `git_status_map` but for a copy-kind subsession with no git
/// history to ask — the best available signal is "this path appeared in one
/// of this session's own file_edits", surfaced as `FileStatus::Touched`
/// rather than claiming to know added/modified/deleted with any certainty.
fn touched_status_map(conn: &rusqlite::Connection, session_id: &str, cwd: &str) -> Result<HashMap<String, FileStatus>, String> {
    let mut stmt = conn
        .prepare("SELECT DISTINCT file_path FROM file_edits WHERE session_id = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![session_id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    let mut map = HashMap::new();
    for abs_path in rows.filter_map(Result::ok) {
        let rel = Path::new(&abs_path)
            .strip_prefix(cwd)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or(abs_path);
        map.insert(rel, FileStatus::Touched);
    }
    Ok(map)
}

/// Recursively walks a directory into a `FileTreeNode` tree, skipping the
/// same heavy/generated directories `copy_project_dir` already excludes
/// (`COPY_SKIP_DIRS`), and annotating each file with its status from the
/// already-resolved status map (git or touched-file fallback).
fn build_file_tree(dir: &Path, rel_prefix: &str, statuses: &HashMap<String, FileStatus>) -> Result<Vec<FileTreeNode>, String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());

    let mut nodes = Vec::new();
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if COPY_SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let rel_path = if rel_prefix.is_empty() { name.clone() } else { format!("{rel_prefix}/{name}") };
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            let children = build_file_tree(&entry.path(), &rel_path, statuses)?;
            nodes.push(FileTreeNode { path: rel_path, name, is_dir: true, status: None, children });
        } else if file_type.is_file() {
            let status = statuses.get(&rel_path).copied();
            nodes.push(FileTreeNode { path: rel_path, name, is_dir: false, status, children: Vec::new() });
        }
    }
    Ok(nodes)
}

/// A deleted file no longer exists on disk, so `build_file_tree`'s real
/// filesystem walk never sees it — this inserts a synthetic leaf node for it
/// at the right nested position (creating placeholder directory nodes along
/// the way if needed), so deletions still show up in the tree, red, like
/// VS Code's explorer does.
fn insert_deleted_path(nodes: &mut Vec<FileTreeNode>, rel_path: &str) {
    let parts: Vec<&str> = rel_path.split('/').collect();
    insert_deleted_path_parts(nodes, &parts, "");
}

fn insert_deleted_path_parts(nodes: &mut Vec<FileTreeNode>, parts: &[&str], prefix: &str) {
    let Some((head, rest)) = parts.split_first() else { return };
    let rel_path = if prefix.is_empty() { head.to_string() } else { format!("{prefix}/{head}") };
    if rest.is_empty() {
        if !nodes.iter().any(|n| n.path == rel_path) {
            nodes.push(FileTreeNode {
                path: rel_path,
                name: head.to_string(),
                is_dir: false,
                status: Some(FileStatus::Deleted),
                children: Vec::new(),
            });
        }
        return;
    }
    if let Some(existing) = nodes.iter_mut().find(|n| n.path == rel_path && n.is_dir) {
        insert_deleted_path_parts(&mut existing.children, rest, &rel_path);
    } else {
        let mut new_dir = FileTreeNode { path: rel_path.clone(), name: head.to_string(), is_dir: true, status: None, children: Vec::new() };
        insert_deleted_path_parts(&mut new_dir.children, rest, &rel_path);
        nodes.push(new_dir);
    }
}

/// Resolves a session's real working directory — its worktree if it's a
/// subsession, otherwise the project's own path (same resolution
/// `send_message` and `list_session_files` use).
fn resolve_session_cwd(conn: &Connection, session_id: &str) -> Result<String, String> {
    let (project_id, worktree_path): (String, Option<String>) = conn
        .query_row(
            "SELECT project_id, worktree_path FROM sessions WHERE id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("session lookup failed: {e}"))?;
    let project_path: String = conn
        .query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
        .map_err(|e| format!("project lookup failed: {e}"))?;
    Ok(worktree_path.unwrap_or(project_path))
}

/// Live file tree for a session's working directory, colored by git status
/// where available, or by "touched in this session" for copy-kind
/// subsessions with no git history.
#[tauri::command]
pub fn list_session_files(db: State<DbConnection>, session_id: String) -> Result<Vec<FileTreeNode>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let cwd = resolve_session_cwd(&conn, &session_id)?;

    if !Path::new(&cwd).is_dir() {
        return Ok(Vec::new());
    }

    let is_git = project_is_git_repo(&cwd);
    let statuses = if is_git { git_status_map(Path::new(&cwd)) } else { touched_status_map(&conn, &session_id, &cwd)? };
    drop(conn);

    let mut tree = build_file_tree(Path::new(&cwd), "", &statuses)?;
    if is_git {
        for (path, status) in &statuses {
            if *status == FileStatus::Deleted {
                insert_deleted_path(&mut tree, path);
            }
        }
    }
    Ok(tree)
}

/// Joins a caller-supplied relative path onto a root **and refuses to leave
/// it**.
///
/// `Path::join` is not a safe primitive for this: given an absolute path it
/// discards the base entirely (`join("/etc/passwd")` *is* `/etc/passwd`), and it
/// happily keeps `..` components. That was fine while the only caller was
/// AgentPanel's own tree, which only ever sends paths it generated itself — and
/// it stops being fine the moment a general file screen makes the path an input.
/// So the check lives here, once, instead of at each call site.
///
/// Both sides are canonicalized, so a symlink pointing out of the tree is caught
/// too, not just a literal `../`.
pub(crate) fn resolve_path_within(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let root = root.canonicalize().map_err(|e| format!("cannot resolve the working directory: {e}"))?;
    let candidate = root.join(rel);
    let resolved = candidate.canonicalize().map_err(|e| format!("cannot resolve {rel}: {e}"))?;
    if !resolved.starts_with(&root) {
        return Err(format!("{rel} is outside this project"));
    }
    Ok(resolved)
}

/// Reads a single file's current content within a session's working
/// directory, for the read-only "File" view in AgentPanel's content panel.
#[tauri::command]
pub fn read_session_file(db: State<DbConnection>, session_id: String, path: String) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let cwd = resolve_session_cwd(&conn, &session_id)?;
    drop(conn);
    let file = resolve_path_within(Path::new(&cwd), &path)?;
    std::fs::read_to_string(file).map_err(|e| e.to_string())
}

/// What a project's git already thinks about a branch you want to work on.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchAvailability {
    pub branch: String,
    /// Whether the branch exists locally at all.
    pub exists: bool,
    /// The worktree (or main checkout) that currently has it checked out.
    /// `Some` means git will refuse to put it anywhere else.
    pub checked_out_at: Option<String>,
}

/// Parses `git worktree list --porcelain` into (path, branch) pairs.
///
/// Pure, because the format is the whole risk: the porcelain output puts each
/// worktree in a block of `worktree <path>` / `HEAD <sha>` / `branch
/// refs/heads/<name>`, with detached ones simply missing the branch line — and
/// a parser that assumed a fixed shape would silently mis-attribute branches.
pub(crate) fn parse_worktree_list(output: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut path: Option<String> = None;
    for line in output.lines() {
        if let Some(rest) = line.strip_prefix("worktree ") {
            path = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            let branch = rest.trim().strip_prefix("refs/heads/").unwrap_or(rest.trim()).to_string();
            if let Some(path) = path.clone() {
                out.push((path, branch));
            }
        }
    }
    out
}

/// Whether a branch can be checked out into a new worktree, and if not, why.
///
/// Asked *before* the user commits to a row rather than after they press
/// launch: git refuses the same branch in two worktrees, and the place it's
/// most likely already open is the user's own main clone — precisely because
/// that's the branch they were working on.
#[tauri::command]
pub fn check_branch_availability(db: State<DbConnection>, project_id: String, branch: String) -> Result<BranchAvailability, String> {
    let project_path: String = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
            .map_err(|e| format!("project lookup failed: {e}"))?
    };
    let dir = Path::new(&project_path);
    let exists = crate::commands::pull_requests::local_branch_exists(dir, &branch);
    let checked_out_at = crate::commands::pull_requests::git_capture(dir, &["worktree", "list", "--porcelain"])
        .ok()
        .and_then(|out| parse_worktree_list(&out).into_iter().find(|(_, b)| *b == branch).map(|(path, _)| path));
    Ok(BranchAvailability { branch, exists, checked_out_at })
}

/// The file tree of any target — a project or a session — for the Files screen.
///
/// Generalizes `list_session_files` the same way the terminals generalized their
/// own target: the screen exists for projects, and a session is just the other
/// kind of working directory. Statuses come from git when there is a repo.
#[tauri::command]
pub fn list_files(db: State<DbConnection>, target: crate::terminals::TerminalTarget) -> Result<Vec<FileTreeNode>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let (cwd, _) = crate::terminals::resolve_target_for_files(&conn, &target)?;
    if !Path::new(&cwd).is_dir() {
        return Ok(Vec::new());
    }

    let is_git = project_is_git_repo(&cwd);
    let statuses = if is_git { git_status_map(Path::new(&cwd)) } else { HashMap::new() };
    drop(conn);

    let mut tree = build_file_tree(Path::new(&cwd), "", &statuses)?;
    if is_git {
        for (path, status) in &statuses {
            if *status == FileStatus::Deleted {
                insert_deleted_path(&mut tree, path);
            }
        }
    }
    Ok(tree)
}

/// One file's current content plus, when git has one, the committed version it
/// differs from — so the Files screen can show the diff without a second round
/// trip and without the frontend having to know how to ask git.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileContents {
    pub text: String,
    /// `HEAD`'s version of the same path. `None` when the file is untracked,
    /// brand new, or there's no repo — in which case there is nothing to diff
    /// against, which the UI shows as "new file" rather than an empty diff.
    pub head_text: Option<String>,
}

#[tauri::command]
pub fn read_file(db: State<DbConnection>, target: crate::terminals::TerminalTarget, path: String) -> Result<FileContents, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let (cwd, _) = crate::terminals::resolve_target_for_files(&conn, &target)?;
    drop(conn);

    let root = Path::new(&cwd);
    let file = resolve_path_within(root, &path)?;
    let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    // `git show HEAD:<path>` fails for an untracked or newly added file, which
    // isn't an error — there's simply no previous version.
    let head_text = crate::commands::pull_requests::git_capture(root, &["show", &format!("HEAD:{path}")]).ok();
    Ok(FileContents { text, head_text })
}

/// Scans this session's Claude Code config dir for other conversations
/// against the same project, so the user can pick one to continue instead of
/// this session's own conversation — there is no `claude -r`-style
/// interactive picker available to a headless caller, so Hangar reads the
/// same on-disk transcripts (`<config_dir>/projects/<slug>/<id>.jsonl`) the
/// real picker draws from.
#[tauri::command]
pub fn list_resumable_conversations(
    db: State<DbConnection>,
    session_id: String,
) -> Result<Vec<ResumableConversation>, String> {
    let (config_dir_path, project_path, current_provider_session_id): (String, String, Option<String>) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let (account_id, project_id, resume, worktree_path): (String, String, Option<String>, Option<String>) = conn
            .query_row(
                "SELECT account_id, project_id, provider_session_id, worktree_path FROM sessions WHERE id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .map_err(|e| format!("session lookup failed: {e}"))?;
        let config_dir_path: String = conn
            .query_row("SELECT config_dir_path FROM accounts WHERE id = ?1", params![account_id], |row| row.get(0))
            .map_err(|e| format!("account lookup failed: {e}"))?;
        let project_path: String = conn
            .query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
            .map_err(|e| format!("project lookup failed: {e}"))?;
        // Match send_message's own cwd resolution — a subsession's own past
        // turns were run (and recorded by Claude Code) from its worktree.
        let project_path = worktree_path.unwrap_or(project_path);
        (config_dir_path, project_path, resume)
    };

    let projects_dir = std::path::Path::new(&config_dir_path).join("projects");
    let mut results = Vec::new();
    let Ok(project_subdirs) = std::fs::read_dir(&projects_dir) else {
        return Ok(results);
    };
    for subdir_entry in project_subdirs.flatten() {
        let subdir_path = subdir_entry.path();
        if !subdir_path.is_dir() {
            continue;
        }
        let Ok(files) = std::fs::read_dir(&subdir_path) else { continue };
        for file_entry in files.flatten() {
            let file_path = file_entry.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(convo) = inspect_transcript(&file_path, &project_path) else { continue };
            if Some(&convo.provider_session_id) == current_provider_session_id.as_ref() {
                continue;
            }
            results.push(convo);
        }
    }
    results.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
    Ok(results)
}

/// Reads just enough of a transcript file to describe it: the conversation's
/// own session id, a preview of its first human message, and whether it
/// belongs to the expected project (transcripts open with bookkeeping lines
/// before the first message, so `cwd` isn't necessarily on line 1).
fn inspect_transcript(path: &std::path::Path, expected_cwd: &str) -> Option<ResumableConversation> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);

    let mut session_id = None;
    let mut preview = None;
    for line in reader.lines().take(20) {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else { continue };

        if let Some(cwd) = value.get("cwd").and_then(|v| v.as_str()) {
            if cwd != expected_cwd {
                return None;
            }
        }
        if session_id.is_none() {
            session_id = value.get("sessionId").and_then(|v| v.as_str()).map(str::to_string);
        }
        if preview.is_none() && value.get("type").and_then(|v| v.as_str()) == Some("user") {
            if let Some(text) = value.pointer("/message/content").and_then(|v| v.as_str()) {
                if !text.trim().is_empty() {
                    let truncated: String = text.trim().chars().take(140).collect();
                    preview = Some(truncated);
                }
            }
        }
        if session_id.is_some() && preview.is_some() {
            break;
        }
    }

    let session_id = session_id?;
    let preview = preview.unwrap_or_else(|| "(no preview available)".to_string());
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let last_active_at: chrono::DateTime<Utc> = modified.into();

    Some(ResumableConversation {
        provider_session_id: session_id,
        preview,
        last_active_at: last_active_at.to_rfc3339(),
    })
}

/// Points this session at a different Claude Code conversation — the next
/// `send_message` call already reads `provider_session_id` from this row and
/// passes it as `--resume`, so nothing else needs to change. Leaves this
/// session's own visible timeline untouched, aside from a marker block so
/// the context switch isn't silent.
#[tauri::command]
pub fn set_session_resume_target(
    app: AppHandle,
    db: State<DbConnection>,
    session_id: String,
    provider_session_id: String,
) -> Result<(), String> {
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE sessions SET provider_session_id = ?2 WHERE id = ?1",
            params![session_id, provider_session_id],
        )
        .map_err(|e| e.to_string())?;
    }
    process_manager::emit_block(
        &app,
        &session_id,
        "agent",
        "↻ resumed a different Claude Code conversation — the next message continues that one instead",
    );
    Ok(())
}

#[cfg(test)]
mod resume_tests {
    use super::*;

    /// Real shape of a Claude Code transcript line for a human message: some
    /// bookkeeping lines with no `cwd` first, then a `"type":"user"` line
    /// with a plain-string `message.content` (not the tool_result array
    /// shape `turn_runner::parse_line` handles) plus `cwd` and `sessionId`.
    #[test]
    fn inspect_transcript_extracts_session_id_and_preview_for_matching_project() {
        let convo = inspect_transcript(
            std::path::Path::new("tests/fixtures/resumable_same_project.jsonl"),
            "/repo/sample-project",
        )
        .expect("expected a resumable conversation for the matching project");
        assert_eq!(convo.provider_session_id, "aaaaaaaa-1111-2222-3333-444444444444");
        assert!(convo.preview.contains("refactor the login flow"));
    }

    #[test]
    fn inspect_transcript_rejects_mismatched_project() {
        let convo = inspect_transcript(
            std::path::Path::new("tests/fixtures/resumable_other_project.jsonl"),
            "/repo/sample-project",
        );
        assert!(convo.is_none());
    }
}

#[cfg(test)]
mod subsession_tests {
    use super::*;
    use tauri::Manager;

    fn make_temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-test-{label}-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn slugify_normalizes_names() {
        assert_eq!(slugify("Fix Billing Bug"), "fix-billing-bug");
        assert_eq!(slugify("  spaces   collapse  "), "spaces-collapse");
        assert_eq!(slugify("weird!!chars??"), "weird-chars");
        assert!(slugify("").starts_with("subsession-"));
        assert!(slugify("   ").starts_with("subsession-"));
    }

    #[test]
    fn unique_worktree_dir_disambiguates_existing_names() {
        let project = make_temp_dir("unique-worktree");
        let worktrees_dir = project.join(WORKTREES_DIR).join("worktrees");
        std::fs::create_dir_all(worktrees_dir.join("fix-bug")).unwrap();

        let picked = unique_worktree_dir(project.to_str().unwrap(), "fix-bug");
        assert_ne!(picked, worktrees_dir.join("fix-bug"), "must not reuse an existing worktree dir");
        assert!(picked.starts_with(&worktrees_dir));

        let _ = std::fs::remove_dir_all(&project);
    }

    /// New worktrees land under `.hangar`, but a slug already used by a
    /// pre-rename `.fleet` worktree is still taken as far as git is concerned:
    /// the branch name comes from the same slug, and `git worktree add -b`
    /// fails outright on a branch that already exists.
    #[test]
    fn a_slug_taken_by_a_pre_rename_worktree_still_gets_disambiguated() {
        let project = make_temp_dir("legacy-worktree");
        std::fs::create_dir_all(project.join(LEGACY_WORKTREES_DIR).join("worktrees").join("fix-bug")).unwrap();

        let picked = unique_worktree_dir(project.to_str().unwrap(), "fix-bug");

        let new_base = project.join(WORKTREES_DIR).join("worktrees");
        assert!(picked.starts_with(&new_base), "new worktrees must go under .hangar, got {picked:?}");
        assert_ne!(picked, new_base.join("fix-bug"), "the slug is taken by the legacy worktree — it must be suffixed");

        let _ = std::fs::remove_dir_all(&project);
    }

    #[test]
    fn project_is_git_repo_detects_real_repos_and_plain_folders() {
        let git_project = make_temp_dir("is-git");
        run_git(&git_project, &["init"]).unwrap();
        assert!(project_is_git_repo(git_project.to_str().unwrap()));

        let plain_project = make_temp_dir("not-git");
        assert!(!project_is_git_repo(plain_project.to_str().unwrap()));

        let _ = std::fs::remove_dir_all(&git_project);
        let _ = std::fs::remove_dir_all(&plain_project);
    }

    /// Real git repo (temp dir + real `git` binary): confirms
    /// `create_linked_worktree` produces a worktree that `git worktree list`
    /// in the project itself actually recognizes, and that
    /// `cleanup_worktree_rows` removes both the directory and the branch.
    #[test]
    fn create_linked_worktree_and_cleanup_round_trip() {
        let project_dir = make_temp_dir("git-project");
        run_git(&project_dir, &["init"]).unwrap();
        std::fs::write(project_dir.join("README.md"), "hello").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "init"]).unwrap();

        let worktree_dir = project_dir.join(WORKTREES_DIR).join("worktrees").join("fix-bug");
        let branch = "subsession/fix-bug".to_string();
        create_linked_worktree(project_dir.to_str().unwrap(), &worktree_dir, &branch, None)
            .expect("creating a linked worktree in a real git repo should succeed");

        assert!(worktree_dir.join("README.md").is_file(), "worktree should have the parent's committed files");
        let list_output = Command::new(providers::resolve_binary("git"))
            .args(["worktree", "list"])
            .current_dir(&project_dir)
            .output()
            .unwrap();
        let listed = String::from_utf8_lossy(&list_output.stdout);
        assert!(listed.contains(worktree_dir.to_str().unwrap()), "git worktree list should show the new worktree: {listed}");

        cleanup_worktree_rows(&[WorktreeCleanupRow {
            worktree_path: worktree_dir.to_string_lossy().to_string(),
            worktree_branch: Some(branch.clone()),
            worktree_kind: "linked".to_string(),
            project_path: project_dir.to_str().unwrap().to_string(),
        }]);

        assert!(!worktree_dir.exists(), "cleanup should remove the worktree directory");
        let branch_list = Command::new(providers::resolve_binary("git"))
            .args(["branch", "--list", &branch])
            .current_dir(&project_dir)
            .output()
            .unwrap();
        assert!(String::from_utf8_lossy(&branch_list.stdout).trim().is_empty(), "cleanup should delete the branch too");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// A project with no git repo: `copy_project_dir` + `init_fresh_git_repo`
    /// give the copy its own history, and `cleanup_worktree_rows` with
    /// kind = "copy" just removes the directory (no branch to delete).
    #[test]
    fn copy_project_dir_and_init_fresh_git_repo_then_cleanup() {
        let project_dir = make_temp_dir("plain-project");
        std::fs::write(project_dir.join("app.txt"), "some project file").unwrap();
        std::fs::create_dir_all(project_dir.join("node_modules")).unwrap();
        std::fs::write(project_dir.join("node_modules").join("junk.js"), "should not be copied").unwrap();

        let worktree_dir = project_dir.join(WORKTREES_DIR).join("worktrees").join("try-something");
        copy_project_dir(&project_dir, &worktree_dir).expect("copying a plain folder should succeed");
        init_fresh_git_repo(&worktree_dir);

        assert!(worktree_dir.join("app.txt").is_file(), "copy should include the project's files");
        assert!(!worktree_dir.join("node_modules").exists(), "heavy dirs like node_modules should be skipped");
        assert!(worktree_dir.join(".git").exists(), "copy should get its own fresh git repo");

        cleanup_worktree_rows(&[WorktreeCleanupRow {
            worktree_path: worktree_dir.to_string_lossy().to_string(),
            worktree_branch: None,
            worktree_kind: "copy".to_string(),
            project_path: project_dir.to_str().unwrap().to_string(),
        }]);
        assert!(!worktree_dir.exists(), "cleanup should remove the copied directory");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    fn setup_test_app_with_account_and_project(project_path: &str) -> (tauri::App<tauri::test::MockRuntime>, String, String) {
        use rusqlite::Connection;
        use std::sync::Mutex as StdMutex;

        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();

        let account_id = "acc1".to_string();
        let project_id = "proj1".to_string();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES (?1,'claude','sub','Test','work','','','acc1','healthy','now','now')",
            params![account_id],
        ).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES (?1,'Test',?2,'now')",
            params![project_id, project_path],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('parent1','parent-session',?1,?2,'sonnet','high','ask_edits','waiting','now')",
            params![project_id, account_id],
        ).unwrap();

        app.manage(crate::db::DbConnection(StdMutex::new(conn)));
        app.manage(crate::process_manager::ProcessRegistry::new());
        app.manage(crate::turn_runner::TurnRegistry::new());
        (app, account_id, project_id)
    }

    /// End-to-end: a real git repo on disk, the actual `create_subsession`
    /// Tauri command, and a real `git worktree list` check — the exact
    /// first manual scenario from the feature's plan (git project → linked
    /// worktree + branch appear, then both disappear on delete).
    #[test]
    /// `git worktree list --porcelain` puts each worktree in a block, and a
    /// detached one simply has no `branch` line. A parser that assumed a fixed
    /// shape would attribute the previous worktree's branch to it.
    #[test]
    fn the_worktree_list_is_parsed_including_detached_entries() {
        let output = "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\nworktree /repo/.hangar/worktrees/t2\nHEAD def456\nbranch refs/heads/T2\n\nworktree /repo/.hangar/worktrees/loose\nHEAD 999\ndetached\n";
        let parsed = parse_worktree_list(output);
        assert_eq!(parsed, vec![("/repo".to_string(), "main".to_string()), ("/repo/.hangar/worktrees/t2".to_string(), "T2".to_string())]);
    }

    /// The case that decides the whole task flow: a branch already checked out
    /// somewhere — usually the user's own main clone, because that's the branch
    /// they were working on — can't go into a second worktree, and the UI has
    /// to know before the user commits to the row.
    #[test]
    fn a_branch_checked_out_elsewhere_is_reported_with_where() {
        let project_dir = make_temp_dir("branch-availability");
        run_git(&project_dir, &["init"]).unwrap();
        std::fs::write(project_dir.join("README.md"), "hi").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "init"]).unwrap();
        run_git(&project_dir, &["branch", "T2"]).unwrap();

        let (app, _account, project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();

        // T2 exists and nobody holds it: free to take.
        let free = check_branch_availability(db.clone(), project_id.clone(), "T2".to_string()).unwrap();
        assert!(free.exists);
        assert_eq!(free.checked_out_at, None);

        // The branch the main clone is on cannot go anywhere else.
        let current = check_branch_availability(db.clone(), project_id.clone(), "main".to_string())
            .or_else(|_| check_branch_availability(db.clone(), project_id.clone(), "master".to_string()))
            .unwrap();
        assert!(current.checked_out_at.is_some(), "the checked-out branch must report where it's held");

        // A name nobody has used yet is neither existing nor held — the
        // "create it" case.
        let fresh = check_branch_availability(db.clone(), project_id, "T7".to_string()).unwrap();
        assert!(!fresh.exists);
        assert_eq!(fresh.checked_out_at, None);

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// A task worktree carries the task's own branch name, not a namespaced
    /// one: that name is what the PR and CI will show.
    #[test]
    fn a_task_worktree_checks_out_the_task_branch_verbatim() {
        let project_dir = make_temp_dir("task-worktree");
        run_git(&project_dir, &["init"]).unwrap();
        std::fs::write(project_dir.join("README.md"), "hi").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "init"]).unwrap();
        run_git(&project_dir, &["branch", "T2"]).unwrap();

        let (app, account_id, project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());

        let session = tauri::async_runtime::block_on(create_worktree_on_branch(
            app.handle().clone(),
            project_id,
            WorktreeBranch::Existing("T2".to_string()),
            account_id,
        ))
        .expect("checking out an existing task branch must succeed");

        assert_eq!(session.worktree_branch.as_deref(), Some("T2"), "no `hangar/` prefix — the branch is the task's");

        let worktree = std::path::PathBuf::from(session.worktree_path.clone().unwrap());
        let head = Command::new(providers::resolve_binary("git"))
            .args(["rev-parse", "--abbrev-ref", "HEAD"])
            .current_dir(&worktree)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "T2", "git inside the worktree must report the task's branch");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// The hole this closes. `Path::join` with an absolute path discards the
    /// base entirely and keeps `..` components, so before this the file reader
    /// would happily serve `/etc/passwd` or `~/.ssh/id_rsa` to anyone who could
    /// name them. It only ever got paths from Hangar's own tree; making the file
    /// viewer general is what turns that into a real hole.
    #[test]
    fn a_path_outside_the_root_is_refused_however_it_is_written() {
        let root = make_temp_dir("path-confinement");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs").as_path(), "fn main() {}").unwrap();
        let secret = root.parent().unwrap().join("outside-secret.txt");
        std::fs::write(&secret, "not yours").unwrap();

        assert!(resolve_path_within(&root, "src/main.rs").is_ok(), "a real file inside the root must still be readable");

        for escape in ["/etc/passwd", "../outside-secret.txt", "src/../../outside-secret.txt"] {
            let err = resolve_path_within(&root, escape).unwrap_err();
            assert!(err.contains("outside this project") || err.contains("cannot resolve"), "'{escape}' must be refused, got: {err}");
        }

        let _ = std::fs::remove_file(&secret);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A symlink is the version of this that a plain string check misses: the
    /// path has no `..` in it at all, and still lands outside.
    #[test]
    fn a_symlink_pointing_out_of_the_root_is_refused_too() {
        let root = make_temp_dir("path-confinement-symlink");
        let secret = root.parent().unwrap().join("symlinked-secret.txt");
        std::fs::write(&secret, "not yours").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, root.join("looks-innocent.txt")).unwrap();

        #[cfg(unix)]
        {
            let err = resolve_path_within(&root, "looks-innocent.txt").unwrap_err();
            assert!(err.contains("outside this project"), "got: {err}");
        }

        let _ = std::fs::remove_file(&secret);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The terminal path: a worktree cut straight from a project, with no
    /// session to branch off. Same machinery, no parent — which is exactly what
    /// used to be impossible.
    #[test]
    fn create_worktree_for_project_needs_no_parent_session() {
        let project_dir = make_temp_dir("e2e-worktree-no-parent");
        run_git(&project_dir, &["init"]).unwrap();
        std::fs::write(project_dir.join("README.md"), "hello").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "init"]).unwrap();

        let (app, account_id, project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();
        // No session at all in this project — the case discovery used to skip
        // and the subsession dialog could never reach.
        db.0.lock().unwrap().execute("DELETE FROM sessions", []).unwrap();

        let worktree = tauri::async_runtime::block_on(create_worktree_for_project(
            app.handle().clone(),
            project_id,
            "Fix Billing".to_string(),
            account_id,
            None,
            None,
        ))
        .expect("a project should be able to grow a worktree with no session behind it");

        assert_eq!(worktree.parent_session_id, None, "there is no parent, and the row must not pretend otherwise");
        assert_eq!(worktree.worktree_kind.as_deref(), Some("linked"));
        assert_eq!(worktree.worktree_branch.as_deref(), Some("hangar/fix-billing"), "terminal worktrees live in their own branch namespace");
        assert_eq!(worktree.effort, TERMINAL_WORKTREE_EFFORT);
        assert_eq!(worktree.mode, TERMINAL_WORKTREE_MODE);

        let worktree_path = worktree.worktree_path.clone().expect("a linked worktree must have a path");
        assert!(std::path::Path::new(&worktree_path).join("README.md").is_file(), "the worktree must be checked out on the project's files");

        // And a terminal opened against it lands in the worktree, not the project.
        let resolved = crate::terminals::resolve_terminal_target(
            db.clone(),
            crate::terminals::TerminalTarget { kind: crate::terminals::TargetKind::Session, id: worktree.id.clone() },
        )
        .unwrap();
        assert_eq!(resolved, worktree_path, "the terminal's cwd is the worktree");

        tauri::async_runtime::block_on(delete_session(app.handle().clone(), worktree.id)).expect("delete_session should succeed");
        assert!(!std::path::Path::new(&worktree_path).exists(), "deleting it must take the worktree directory with it");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// A worktree is a git object. Without a repo there's nothing to branch
    /// from, no base to choose and no diff to review at the end — so the
    /// terminal path refuses instead of quietly copying the folder into
    /// something that looks like a worktree and behaves like a photocopy.
    /// (`create_subsession`, the agent path, still copies: not this change.)
    #[test]
    fn a_project_without_git_cannot_get_a_worktree_and_says_why() {
        let project_dir = make_temp_dir("e2e-worktree-no-git");
        std::fs::write(project_dir.join("notes.txt"), "no repo here").unwrap();

        let (app, account_id, project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());

        let err = tauri::async_runtime::block_on(create_worktree_for_project(
            app.handle().clone(),
            project_id,
            "scratch".to_string(),
            account_id,
            None,
            None,
        ))
        .expect_err("a folder with no repo must be refused, not copied");

        assert!(err.contains("isn't a git repository"), "the message has to say what's wrong: {err}");
        assert!(err.contains("git init"), "and what to do about it: {err}");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// Naming the base is the point of listing branches: a worktree cut off
    /// whatever happened to be checked out inherits half-finished work, and the
    /// agent's diff then contains changes that aren't its own.
    #[test]
    fn a_worktree_starts_from_the_base_branch_it_was_given() {
        let project_dir = make_temp_dir("e2e-worktree-base");
        run_git(&project_dir, &["init"]).unwrap();
        std::fs::write(project_dir.join("README.md"), "on main").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "init"]).unwrap();
        // A second branch with a file main doesn't have, then back to main.
        run_git(&project_dir, &["checkout", "-b", "release"]).unwrap();
        std::fs::write(project_dir.join("only-on-release.txt"), "release only").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "release work"]).unwrap();
        let main = run_git(&project_dir, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|_| ()).and(Ok::<(), String>(())).is_ok();
        assert!(main);
        run_git(&project_dir, &["checkout", "-"]).unwrap();

        let (app, account_id, project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());

        let worktree = tauri::async_runtime::block_on(create_worktree_for_project(
            app.handle().clone(),
            project_id,
            "from release".to_string(),
            account_id,
            None,
            Some("release".to_string()),
        ))
        .expect("branching from an existing branch must succeed");

        let path = std::path::PathBuf::from(worktree.worktree_path.clone().unwrap());
        assert!(path.join("only-on-release.txt").is_file(), "the worktree must carry the base branch's content, not the checked-out branch's");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    fn create_subsession_end_to_end_for_a_real_git_project() {
        let project_dir = make_temp_dir("e2e-git-project");
        run_git(&project_dir, &["init"]).unwrap();
        std::fs::write(project_dir.join("README.md"), "hello").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "init"]).unwrap();

        let (app, account_id, _project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();

        let subsession = tauri::async_runtime::block_on(create_subsession(
            app.handle().clone(),
            NewSubsession {
                parent_session_id: "parent1".to_string(),
                name: "Try Alt Approach".to_string(),
                account_id,
                model: "sonnet".to_string(),
                effort: "high".to_string(),
                mode: "ask_edits".to_string(),
                setup_command: None,
            },
        ))
        .expect("create_subsession should succeed for a real git project");

        assert_eq!(subsession.worktree_kind.as_deref(), Some("linked"));
        assert_eq!(subsession.parent_session_id.as_deref(), Some("parent1"));
        let worktree_path = subsession.worktree_path.clone().expect("linked subsession must have a worktree_path");
        let worktree_dir = std::path::Path::new(&worktree_path);
        assert!(worktree_dir.join("README.md").is_file(), "worktree should check out the parent repo's committed files");

        let list_output = Command::new(providers::resolve_binary("git"))
            .args(["worktree", "list"])
            .current_dir(&project_dir)
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&list_output.stdout).contains(&worktree_path),
            "git worktree list in the real project must show the new worktree"
        );

        tauri::async_runtime::block_on(delete_session(app.handle().clone(), subsession.id.clone())).expect("delete_session should succeed");

        assert!(!worktree_dir.exists(), "deleting the subsession must remove its worktree directory");
        let remaining: i64 = db.0.lock().unwrap().query_row("SELECT COUNT(*) FROM sessions WHERE id = ?1", params![subsession.id], |r| r.get(0)).unwrap();
        assert_eq!(remaining, 0, "the subsession row must be gone after delete_session");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// End-to-end for a project with no git repo: `create_subsession` must
    /// still succeed by copying the folder and giving it a fresh repo, and
    /// the agent must actually be able to run git commands inside it — the
    /// second manual scenario from the plan.
    #[test]
    fn create_subsession_end_to_end_for_a_project_without_git() {
        let project_dir = make_temp_dir("e2e-plain-project");
        std::fs::write(project_dir.join("app.txt"), "some file").unwrap();

        let (app, account_id, _project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());

        let subsession = tauri::async_runtime::block_on(create_subsession(
            app.handle().clone(),
            NewSubsession {
                parent_session_id: "parent1".to_string(),
                name: "no git here".to_string(),
                account_id,
                model: "sonnet".to_string(),
                effort: "high".to_string(),
                mode: "ask_edits".to_string(),
                setup_command: None,
            },
        ))
        .expect("create_subsession should succeed even without a git repo");

        assert_eq!(subsession.worktree_kind.as_deref(), Some("copy"));
        assert!(subsession.worktree_branch.is_none(), "a copy-kind subsession has no branch");
        let worktree_dir = std::path::PathBuf::from(subsession.worktree_path.clone().unwrap());
        assert!(worktree_dir.join("app.txt").is_file());

        // The agent must actually be able to run git commands in the copy.
        let status = Command::new(providers::resolve_binary("git"))
            .args(["status"])
            .current_dir(&worktree_dir)
            .output()
            .expect("git status should run inside the copied worktree");
        assert!(status.status.success(), "git status must succeed inside a freshly-initialized copy worktree");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// Deleting a parent session with two subsessions must cascade both the
    /// DB rows and the physical worktree folders — the third manual scenario
    /// from the plan.
    #[test]
    fn delete_session_cascades_worktrees_for_all_subsessions() {
        let project_dir = make_temp_dir("e2e-cascade-project");
        run_git(&project_dir, &["init"]).unwrap();
        std::fs::write(project_dir.join("README.md"), "hello").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "init"]).unwrap();

        let (app, account_id, _project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();

        let mut worktree_dirs = Vec::new();
        for name in ["sub-a", "sub-b"] {
            let subsession = tauri::async_runtime::block_on(create_subsession(
                app.handle().clone(),
                NewSubsession {
                    parent_session_id: "parent1".to_string(),
                    name: name.to_string(),
                    account_id: account_id.clone(),
                    model: "sonnet".to_string(),
                    effort: "high".to_string(),
                    mode: "ask_edits".to_string(),
                    setup_command: None,
                },
            ))
            .expect("each subsession should be created successfully");
            worktree_dirs.push(std::path::PathBuf::from(subsession.worktree_path.unwrap()));
        }
        assert!(worktree_dirs.iter().all(|d| d.exists()), "both worktrees should exist before delete");

        tauri::async_runtime::block_on(delete_session(app.handle().clone(), "parent1".to_string()))
            .expect("deleting the parent should succeed and cascade");

        assert!(worktree_dirs.iter().all(|d| !d.exists()), "deleting the parent must remove every subsession's worktree");
        let remaining: i64 = db.0.lock().unwrap().query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0)).unwrap();
        assert_eq!(remaining, 0, "the parent and both subsessions must all be gone from the DB");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// End-to-end against a real git repo: an untracked new file, a modified
    /// tracked file, and a deleted tracked file must each surface with the
    /// right `FileStatus` — including the deleted one, which no longer
    /// exists on disk and only `insert_deleted_path` can recover.
    #[test]
    fn list_session_files_reflects_real_git_status() {
        let project_dir = make_temp_dir("filetree-git-project");
        run_git(&project_dir, &["init"]).unwrap();
        std::fs::write(project_dir.join("tracked.txt"), "original\n").unwrap();
        std::fs::write(project_dir.join("to-delete.txt"), "bye\n").unwrap();
        run_git(&project_dir, &["add", "-A"]).unwrap();
        run_git(&project_dir, &["commit", "-m", "init"]).unwrap();

        // Now make the working tree dirty: modify one tracked file, delete
        // another, and add a brand-new untracked file.
        std::fs::write(project_dir.join("tracked.txt"), "changed\n").unwrap();
        std::fs::remove_file(project_dir.join("to-delete.txt")).unwrap();
        std::fs::write(project_dir.join("new-file.txt"), "hello\n").unwrap();

        let (app, _account_id, _project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();

        let tree = list_session_files(db.clone(), "parent1".to_string()).expect("list_session_files should succeed");
        let find = |name: &str| tree.iter().find(|n| n.name == name);

        assert_eq!(find("tracked.txt").map(|n| n.status), Some(Some(FileStatus::Modified)));
        assert_eq!(find("new-file.txt").map(|n| n.status), Some(Some(FileStatus::Untracked)));
        assert_eq!(find("to-delete.txt").map(|n| n.status), Some(Some(FileStatus::Deleted)), "a deleted file must still appear in the tree, marked deleted");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// A project with no git repo falls back to marking paths recorded in
    /// `file_edits` as `Touched` rather than claiming a git status it can't
    /// actually know.
    #[test]
    fn list_session_files_falls_back_to_touched_status_without_git() {
        let project_dir = make_temp_dir("filetree-plain-project");
        std::fs::write(project_dir.join("app.txt"), "hello\n").unwrap();
        std::fs::write(project_dir.join("untouched.txt"), "unrelated\n").unwrap();

        let (app, _account_id, _project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO file_edits (id, session_id, block_id, file_path, tool_name, old_text, new_text, created_at)
                 VALUES ('fe1','parent1',NULL,?1,'Edit','old','new','now')",
                params![project_dir.join("app.txt").to_string_lossy().to_string()],
            )
            .unwrap();
        }

        let tree = list_session_files(db.clone(), "parent1".to_string()).expect("list_session_files should succeed without git");
        let find = |name: &str| tree.iter().find(|n| n.name == name);

        assert_eq!(find("app.txt").map(|n| n.status), Some(Some(FileStatus::Touched)));
        assert_eq!(find("untouched.txt").map(|n| n.status), Some(None), "a file never edited this session has no status");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    /// `COPY_SKIP_DIRS` entries (node_modules, .git, etc.) must not appear in
    /// the tree at all — same exclusion list `copy_project_dir` already uses.
    #[test]
    fn list_session_files_skips_heavy_directories() {
        let project_dir = make_temp_dir("filetree-skip-dirs");
        std::fs::create_dir_all(project_dir.join("node_modules")).unwrap();
        std::fs::write(project_dir.join("node_modules").join("junk.js"), "noise").unwrap();
        std::fs::write(project_dir.join("real.txt"), "keep me").unwrap();

        let (app, _account_id, _project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();

        let tree = list_session_files(db.clone(), "parent1".to_string()).unwrap();
        assert!(!tree.iter().any(|n| n.name == "node_modules"), "node_modules must be excluded from the tree");
        assert!(tree.iter().any(|n| n.name == "real.txt"));

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn read_session_file_returns_exact_content() {
        let project_dir = make_temp_dir("read-file-project");
        std::fs::write(project_dir.join("hello.txt"), "hello world\n").unwrap();

        let (app, _account_id, _project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();

        let content = read_session_file(db.clone(), "parent1".to_string(), "hello.txt".to_string())
            .expect("read_session_file should succeed for an existing file");
        assert_eq!(content, "hello world\n");

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn read_session_file_errs_on_missing_path() {
        let project_dir = make_temp_dir("read-file-missing");
        std::fs::write(project_dir.join("real.txt"), "keep me").unwrap();

        let (app, _account_id, _project_id) = setup_test_app_with_account_and_project(project_dir.to_str().unwrap());
        let db = app.state::<DbConnection>();

        let result = read_session_file(db.clone(), "parent1".to_string(), "does-not-exist.txt".to_string());
        assert!(result.is_err(), "reading a nonexistent file must return Err, not panic");

        let _ = std::fs::remove_dir_all(&project_dir);
    }
}
