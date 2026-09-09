use crate::commands::accounts::row_to_account;
use crate::commands::sessions::{row_to_session, COPY_SKIP_DIRS};
use crate::credential_vault;
use crate::db::DbConnection;
use crate::models::{
    Account, AppSettings, DiscoveredMemoryFile, GroupMembership, MemoryFile, MemoryFileSummary, MemoryGroup, MemoryGroupSummary,
    ProjectAccountRotation, Session,
};
use crate::providers::{claude, Mode};
use crate::turn_runner::{self, TurnRequest};
use chrono::Utc;
use rusqlite::params;
use std::collections::HashSet;
use std::path::Path;
use tauri::{AppHandle, Manager, Runtime, State};
use uuid::Uuid;

/// The only account field Settings/General needs to edit after creation —
/// deliberately narrow rather than a generic `update_account`, since nothing
/// else on the account is editable post-creation today.
#[tauri::command]
pub fn set_account_monthly_cap(db: State<DbConnection>, id: String, monthly_cap: Option<f64>) -> Result<Account, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE accounts SET monthly_cap = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, monthly_cap, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM accounts WHERE id = ?1", params![id], row_to_account).map_err(|e| e.to_string())
}

pub(crate) fn read_app_settings(conn: &rusqlite::Connection) -> Result<AppSettings, String> {
    let mut stmt = conn.prepare("SELECT key, value FROM app_settings").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))).map_err(|e| e.to_string())?;
    let mut settings = AppSettings {
        reviewer_enabled: false,
        reviewer_account_id: None,
        notify_needs_approval: false,
        notify_session_waiting: false,
        notify_account_exhausted: false,
        notify_spend_cap: false,
        notify_pr_comments: false,
        notify_update_available: false,
        agent_trees_enabled: false,
        include_own_pr_comments: false,
        // The value this shipped with, so an install that has never touched the
        // setting behaves exactly as it did before it existed.
        panel_warn_threshold: 4,
    };
    for row in rows {
        let (key, value) = row.map_err(|e| e.to_string())?;
        match key.as_str() {
            "reviewer_enabled" => settings.reviewer_enabled = value == "1",
            "reviewer_account_id" => settings.reviewer_account_id = if value.is_empty() { None } else { Some(value) },
            "notify_needs_approval" => settings.notify_needs_approval = value == "1",
            "notify_session_waiting" => settings.notify_session_waiting = value == "1",
            "notify_account_exhausted" => settings.notify_account_exhausted = value == "1",
            "notify_spend_cap" => settings.notify_spend_cap = value == "1",
            "notify_pr_comments" => settings.notify_pr_comments = value == "1",
            "notify_update_available" => settings.notify_update_available = value == "1",
            "agent_trees_enabled" => settings.agent_trees_enabled = value == "1",
            "include_own_pr_comments" => settings.include_own_pr_comments = value == "1",
            // A row that cannot be parsed leaves the default standing rather
            // than silently turning the warning off.
            "panel_warn_threshold" => {
                if let Ok(n) = value.parse::<i64>() {
                    settings.panel_warn_threshold = n.max(0);
                }
            }
            _ => {}
        }
    }
    Ok(settings)
}

fn write_setting(conn: &rusqlite::Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_app_settings(db: State<DbConnection>) -> Result<AppSettings, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    read_app_settings(&conn)
}

#[tauri::command]
pub fn set_app_settings(db: State<DbConnection>, input: AppSettings) -> Result<AppSettings, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let bool_str = |b: bool| if b { "1" } else { "0" };
    write_setting(&conn, "reviewer_enabled", bool_str(input.reviewer_enabled))?;
    write_setting(&conn, "reviewer_account_id", input.reviewer_account_id.as_deref().unwrap_or(""))?;
    write_setting(&conn, "notify_needs_approval", bool_str(input.notify_needs_approval))?;
    write_setting(&conn, "notify_session_waiting", bool_str(input.notify_session_waiting))?;
    write_setting(&conn, "notify_account_exhausted", bool_str(input.notify_account_exhausted))?;
    write_setting(&conn, "notify_spend_cap", bool_str(input.notify_spend_cap))?;
    write_setting(&conn, "notify_pr_comments", bool_str(input.notify_pr_comments))?;
    write_setting(&conn, "notify_update_available", bool_str(input.notify_update_available))?;
    write_setting(&conn, "agent_trees_enabled", bool_str(input.agent_trees_enabled))?;
    write_setting(&conn, "include_own_pr_comments", bool_str(input.include_own_pr_comments))?;
    write_setting(&conn, "panel_warn_threshold", &input.panel_warn_threshold.max(0).to_string())?;
    Ok(input)
}

/// Every account plus whether `project_id` may use it — a left join against
/// `project_allowed_accounts` so an account with no row yet still shows up
/// (unchecked), matching "opt-in only, nothing shared by default".
#[tauri::command]
pub fn list_project_account_rotation(db: State<DbConnection>, project_id: String) -> Result<Vec<ProjectAccountRotation>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT a.id, a.name, a.provider, a.scope, COALESCE(paa.allowed, 0)
             FROM accounts a
             LEFT JOIN project_allowed_accounts paa ON paa.account_id = a.id AND paa.project_id = ?1
             ORDER BY a.scope, a.name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(ProjectAccountRotation {
                account_id: row.get(0)?,
                account_name: row.get(1)?,
                provider: row.get(2)?,
                scope: row.get(3)?,
                allowed: row.get::<_, i64>(4)? != 0,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_project_account_allowed(db: State<DbConnection>, project_id: String, account_id: String, allowed: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO project_allowed_accounts (project_id, account_id, allowed) VALUES (?1, ?2, ?3)
         ON CONFLICT(project_id, account_id) DO UPDATE SET allowed = excluded.allowed",
        params![project_id, account_id, allowed as i64],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_memory_groups(db: State<DbConnection>) -> Result<Vec<MemoryGroupSummary>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT g.id, g.name,
                    (SELECT COUNT(*) FROM memory_group_projects mgp WHERE mgp.group_id = g.id),
                    (SELECT COUNT(*) FROM memory_files mf
                       JOIN memory_group_projects mgp2 ON mgp2.project_id = mf.project_id
                       WHERE mgp2.group_id = g.id)
             FROM memory_groups g ORDER BY g.created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(MemoryGroupSummary { id: row.get(0)?, name: row.get(1)?, project_count: row.get(2)?, file_count: row.get(3)? })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn create_memory_group(db: State<DbConnection>, name: String) -> Result<MemoryGroup, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let name = if name.trim().is_empty() { "New group".to_string() } else { name.trim().to_string() };
    conn.execute("INSERT INTO memory_groups (id, name, created_at) VALUES (?1, ?2, ?3)", params![id, name, now])
        .map_err(|e| e.to_string())?;
    Ok(MemoryGroup { id, name, created_at: now })
}

/// Only removes the group and its membership rows (`memory_group_projects`
/// cascades) — the memory files themselves belong to their projects, not to
/// the group, so they survive and stay visible in any other group they're
/// also a member of.
#[tauri::command]
pub fn delete_memory_group(db: State<DbConnection>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM memory_groups WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Every project plus whether it belongs to `group_id` — same left-join
/// shape as `list_project_account_rotation`, for the same reason.
#[tauri::command]
pub fn list_group_membership(db: State<DbConnection>, group_id: String) -> Result<Vec<GroupMembership>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT p.id, p.name, (mgp.project_id IS NOT NULL)
             FROM projects p
             LEFT JOIN memory_group_projects mgp ON mgp.project_id = p.id AND mgp.group_id = ?1
             ORDER BY p.name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![group_id], |row| {
            Ok(GroupMembership { project_id: row.get(0)?, project_name: row.get(1)?, member: row.get::<_, i64>(2)? != 0 })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_group_membership(db: State<DbConnection>, group_id: String, project_id: String, member: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if member {
        conn.execute("INSERT OR IGNORE INTO memory_group_projects (group_id, project_id) VALUES (?1, ?2)", params![group_id, project_id])
            .map_err(|e| e.to_string())?;
    } else {
        conn.execute("DELETE FROM memory_group_projects WHERE group_id = ?1 AND project_id = ?2", params![group_id, project_id])
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The union of memory files across every project in `group_id` — the
/// "agents read the union of its groups' memory" behavior the mockup
/// describes, at least for browsing (actual context injection is a later
/// phase, not part of this pass).
#[tauri::command]
pub fn list_group_memory_files(db: State<DbConnection>, group_id: String) -> Result<Vec<MemoryFileSummary>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT mf.id, mf.project_id, p.name, mf.filename, mf.updated_at
             FROM memory_files mf
             JOIN projects p ON p.id = mf.project_id
             JOIN memory_group_projects mgp ON mgp.project_id = mf.project_id
             WHERE mgp.group_id = ?1
             ORDER BY mf.updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![group_id], |row| {
            Ok(MemoryFileSummary { id: row.get(0)?, project_id: row.get(1)?, project_name: row.get(2)?, filename: row.get(3)?, updated_at: row.get(4)? })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn get_memory_file(db: State<DbConnection>, file_id: String) -> Result<MemoryFile, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT mf.id, mf.project_id, p.name, mf.filename, mf.content_md, mf.updated_at
         FROM memory_files mf JOIN projects p ON p.id = mf.project_id
         WHERE mf.id = ?1",
        params![file_id],
        |row| {
            Ok(MemoryFile {
                id: row.get(0)?,
                project_id: row.get(1)?,
                project_name: row.get(2)?,
                filename: row.get(3)?,
                content_md: row.get(4)?,
                updated_at: row.get(5)?,
            })
        },
    )
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn update_memory_file_content(db: State<DbConnection>, file_id: String, content_md: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE memory_files SET content_md = ?2, updated_at = ?3 WHERE id = ?1",
        params![file_id, content_md, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn delete_memory_file(db: State<DbConnection>, file_id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM memory_files WHERE id = ?1", params![file_id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn create_memory_file(db: State<DbConnection>, project_id: String, filename: String, content_md: String) -> Result<MemoryFile, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let filename = if filename.trim().is_empty() { "notes.md".to_string() } else { filename.trim().to_string() };
    conn.execute(
        "INSERT INTO memory_files (id, project_id, filename, content_md, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![id, project_id, filename, content_md, now],
    )
    .map_err(|e| e.to_string())?;
    let project_name: String =
        conn.query_row("SELECT name FROM projects WHERE id = ?1", params![project_id], |row| row.get(0)).map_err(|e| e.to_string())?;
    Ok(MemoryFile { id, project_id, project_name, filename, content_md, updated_at: now })
}

fn collect_markdown_files(dir: &Path, rel_prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if COPY_SKIP_DIRS.contains(&name.as_str()) {
            continue;
        }
        let rel_path = if rel_prefix.is_empty() { name.clone() } else { format!("{rel_prefix}/{name}") };
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            collect_markdown_files(&entry.path(), &rel_path, out);
        } else if file_type.is_file() && name.to_lowercase().ends_with(".md") {
            out.push(rel_path);
        }
    }
}

/// Recursively scans every project's directory for `.md` files not already
/// imported into `memory_files` (same skip-list as subsession copies —
/// `node_modules`/`.git`/`dist`/etc. — see `COPY_SKIP_DIRS`), so Shared
/// Memory can offer real docs that already exist (README.md, CLAUDE.md,
/// docs/*.md, ...) as one-click imports instead of only the blank "+" flow.
#[tauri::command]
pub fn list_discoverable_memory_files(db: State<DbConnection>) -> Result<Vec<DiscoveredMemoryFile>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let projects: Vec<(String, String, String)> = {
        let mut stmt = conn.prepare("SELECT id, name, path FROM projects ORDER BY name").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    let already_imported: HashSet<(String, String)> = {
        let mut stmt = conn.prepare("SELECT project_id, filename FROM memory_files").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };
    drop(conn);

    let mut discovered = Vec::new();
    for (project_id, project_name, project_path) in projects {
        if !Path::new(&project_path).is_dir() {
            continue;
        }
        let mut found = Vec::new();
        collect_markdown_files(Path::new(&project_path), "", &mut found);
        for relative_path in found {
            if already_imported.contains(&(project_id.clone(), relative_path.clone())) {
                continue;
            }
            discovered.push(DiscoveredMemoryFile { project_id: project_id.clone(), project_name: project_name.clone(), relative_path });
        }
    }
    Ok(discovered)
}

/// Reads a discovered file's real content off disk and hands off to
/// `create_memory_file` — the relative path becomes the memory file's
/// `filename` (e.g. `docs/architecture.md`), so files with the same base
/// name in different folders don't collide.
#[tauri::command]
pub fn import_discovered_memory_file(db: State<DbConnection>, project_id: String, relative_path: String) -> Result<MemoryFile, String> {
    let project_path: String = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0)).map_err(|e| e.to_string())?
    };
    let content_md = std::fs::read_to_string(Path::new(&project_path).join(&relative_path))
        .map_err(|e| format!("could not read {relative_path}: {e}"))?;
    create_memory_file(db, project_id, relative_path, content_md)
}

/// "Review now" — spawns a new session, owned by the configured reviewer
/// account, in the exact same working directory as `session_id` (no new git
/// worktree; a review doesn't need to branch further), running a fixed
/// review prompt immediately (unlike a normal new session, which waits for
/// the user to type the first message). `mode: Mode::Plan` so the reviewer
/// can only read/report, never edit.
/// Off the UI thread: it ends in `spawn_turn`, which starts the reviewer's
/// process (see `send_message`, which had the same problem).
#[tauri::command]
pub async fn trigger_review<R: Runtime>(app: AppHandle<R>, session_id: String) -> Result<Session, String> {
    crate::blocking::run_blocking(move || trigger_review_blocking(app, session_id)).await?
}

fn trigger_review_blocking<R: Runtime>(app: AppHandle<R>, session_id: String) -> Result<Session, String> {
    let db = app.state::<DbConnection>();
    let settings = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        read_app_settings(&conn)?
    };
    if !settings.reviewer_enabled {
        return Err("reviewer is disabled in Settings → General".to_string());
    }
    let reviewer_account_id = settings.reviewer_account_id.ok_or("no reviewer account configured in Settings → General")?;

    let (project_id, worktree_path, worktree_branch, worktree_kind, target_name): (String, Option<String>, Option<String>, Option<String>, String) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT project_id, worktree_path, worktree_branch, worktree_kind, name FROM sessions WHERE id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .map_err(|e| format!("session lookup failed: {e}"))?
    };
    let project_path: String = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
            .map_err(|e| format!("project lookup failed: {e}"))?
    };
    let cwd = worktree_path.clone().unwrap_or_else(|| project_path.clone());

    let (provider, billing, config_dir_path): (String, String, String) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT provider, billing, config_dir_path FROM accounts WHERE id = ?1",
            params![reviewer_account_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|e| format!("reviewer account lookup failed: {e}"))?
    };
    let api_key = if billing == "key" { Some(credential_vault::get(&reviewer_account_id)?) } else { None };
    let env = claude::env_for_account(&config_dir_path, api_key.as_deref());

    let new_session_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    let name = format!("Review: {target_name}");
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, node_id, parent_session_id, worktree_path, worktree_branch, worktree_kind, account_id, model, effort, mode, status, pid, provider_session_id, started_at, ended_at)
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, '', 'high', 'plan', 'waiting', NULL, NULL, ?9, NULL)",
            params![new_session_id, name, project_id, session_id, worktree_path, worktree_branch, worktree_kind, reviewer_account_id, now],
        )
        .map_err(|e| e.to_string())?;
    }

    let review_prompt = "Review the changes in this working directory (compare against the base branch with `git diff`) \
         and report issues, risks, or suggestions. Do not make any changes yourself."
        .to_string();

    // The review is prose the user reads, so it obeys the same language
    // preference as any other turn. Best-effort: a review in the wrong
    // language beats no review.
    let instructions = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::app_data::root(&app)
            .and_then(|root| crate::commands::preferences::instruction_file(&root, &conn, Some(&project_id)))
            .unwrap_or_else(|e| {
                log::warn!("trigger_review: could not prepare the agent preferences — {e}");
                None
            })
            // The runner is Claude-only, and Claude takes the file path.
            .map(|resolved| resolved.file)
    };
    // A review reads the diff and the repo it sits in, so it gets the same
    // graph the working session gets — or nothing, which is the common case.
    let mcp_config = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::commands::graph::mcp_config_for_project(&app, &conn, &project_id)
    };
    // A reviewer with the project's skills is the whole point of having a
    // review skill; without this it ran with none and said nothing about it.
    let skills_dir = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::app_data::root(&app)
            .and_then(|root| crate::commands::skills::skills_dir(&root, &conn, &project_id))
            .unwrap_or_else(|e| {
                log::warn!("trigger_review: could not prepare this project's skills — {e}");
                None
            })
    };

    // Cloned rather than moved: `db` is borrowed from this handle and is still
    // needed below, to read back the session this just created.
    turn_runner::spawn_turn(TurnRequest {
        app: app.clone(),
        session_id: new_session_id.clone(),
        account_id: reviewer_account_id,
        provider,
        cwd,
        env,
        mode: Mode::Plan,
        model: String::new(),
        effort: "high".to_string(),
        name,
        resume: None,
        prompt: review_prompt,
        instructions,
        mcp_config,
        skills_dir,
    })?;

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM sessions WHERE id = ?1", params![new_session_id], row_to_session).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex as StdMutex;
    use tauri::Manager;

    /// Same shape as `sessions.rs`'s own test helper (real schema, in-memory
    /// DB, one account + one project) but local to this module — two work
    /// accounts + one project, since Rotation/reviewer tests need more than
    /// one account to choose between.
    fn setup_test_app() -> (tauri::App<tauri::test::MockRuntime>, String, String, String) {
        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();

        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Personal','personal','','','acc1','healthy','now','now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc2','claude','key','Acme Corp','work','','','acc2','healthy','now','now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp/proj1','now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('sess1','test-session','proj1','acc1','sonnet','high','ask_edits','waiting','now')",
            [],
        )
        .unwrap();

        app.manage(crate::db::DbConnection(StdMutex::new(conn)));
        (app, "acc1".to_string(), "acc2".to_string(), "proj1".to_string())
    }

    #[test]
    fn set_account_monthly_cap_updates_and_returns_the_account() {
        let (app, acc1, _acc2, _proj1) = setup_test_app();
        let db = app.state::<DbConnection>();

        let updated = set_account_monthly_cap(db.clone(), acc1.clone(), Some(200.0)).unwrap();
        assert_eq!(updated.monthly_cap, Some(200.0));

        let cleared = set_account_monthly_cap(db.clone(), acc1, None).unwrap();
        assert_eq!(cleared.monthly_cap, None);
    }

    #[test]
    fn app_settings_round_trip_through_get_and_set() {
        let (app, acc1, _acc2, _proj1) = setup_test_app();
        let db = app.state::<DbConnection>();

        let initial = get_app_settings(db.clone()).unwrap();
        assert!(!initial.reviewer_enabled);
        assert_eq!(initial.reviewer_account_id, None);

        let saved = set_app_settings(db.clone(), AppSettings { reviewer_enabled: true, reviewer_account_id: Some(acc1.clone()), ..Default::default() }).unwrap();
        assert!(saved.reviewer_enabled);
        assert_eq!(saved.reviewer_account_id, Some(acc1));

        let reloaded = get_app_settings(db).unwrap();
        assert!(reloaded.reviewer_enabled);
    }

    #[test]
    fn project_account_rotation_defaults_unchecked_then_reflects_updates() {
        let (app, acc1, acc2, proj1) = setup_test_app();
        let db = app.state::<DbConnection>();

        let before = list_project_account_rotation(db.clone(), proj1.clone()).unwrap();
        assert_eq!(before.len(), 2, "both accounts must appear even with no allowlist rows yet");
        assert!(before.iter().all(|r| !r.allowed), "opt-in only — nothing allowed by default");

        set_project_account_allowed(db.clone(), proj1.clone(), acc1.clone(), true).unwrap();
        let after = list_project_account_rotation(db.clone(), proj1.clone()).unwrap();
        let acc1_row = after.iter().find(|r| r.account_id == acc1).unwrap();
        let acc2_row = after.iter().find(|r| r.account_id == acc2).unwrap();
        assert!(acc1_row.allowed);
        assert!(!acc2_row.allowed);

        // Flipping back off must actually take (upsert, not insert-only).
        set_project_account_allowed(db.clone(), proj1.clone(), acc1.clone(), false).unwrap();
        let toggled_off = list_project_account_rotation(db, proj1).unwrap();
        assert!(!toggled_off.iter().find(|r| r.account_id == acc1).unwrap().allowed);
    }

    #[test]
    fn memory_group_membership_and_file_union_across_member_projects() {
        let (app, _acc1, _acc2, proj1) = setup_test_app();
        let db = app.state::<DbConnection>();
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at) VALUES ('proj2','Second','/tmp/proj2','now')",
                [],
            )
            .unwrap();
        }

        let group = create_memory_group(db.clone(), "Personal projects".to_string()).unwrap();
        set_group_membership(db.clone(), group.id.clone(), proj1.clone(), true).unwrap();
        set_group_membership(db.clone(), group.id.clone(), "proj2".to_string(), true).unwrap();

        let membership = list_group_membership(db.clone(), group.id.clone()).unwrap();
        assert_eq!(membership.len(), 2);
        assert!(membership.iter().all(|m| m.member));

        create_memory_file(db.clone(), proj1, "conventions.md".to_string(), "# hi".to_string()).unwrap();
        create_memory_file(db.clone(), "proj2".to_string(), "decisions.md".to_string(), "# log".to_string()).unwrap();

        let files = list_group_memory_files(db.clone(), group.id.clone()).unwrap();
        assert_eq!(files.len(), 2, "files from every member project must show up, not just one");

        let summary = list_memory_groups(db).unwrap();
        let g = summary.iter().find(|g| g.id == group.id).unwrap();
        assert_eq!(g.project_count, 2);
        assert_eq!(g.file_count, 2);
    }

    #[test]
    fn get_and_update_memory_file_content_round_trips() {
        let (app, _acc1, _acc2, proj1) = setup_test_app();
        let db = app.state::<DbConnection>();
        let file = create_memory_file(db.clone(), proj1, "notes.md".to_string(), "first".to_string()).unwrap();

        update_memory_file_content(db.clone(), file.id.clone(), "second".to_string()).unwrap();
        let reloaded = get_memory_file(db, file.id).unwrap();
        assert_eq!(reloaded.content_md, "second");
    }

    #[test]
    fn delete_memory_group_removes_membership_but_keeps_the_files() {
        let (app, _acc1, _acc2, proj1) = setup_test_app();
        let db = app.state::<DbConnection>();
        let group = create_memory_group(db.clone(), "Personal projects".to_string()).unwrap();
        set_group_membership(db.clone(), group.id.clone(), proj1.clone(), true).unwrap();
        let file = create_memory_file(db.clone(), proj1.clone(), "conventions.md".to_string(), "# hi".to_string()).unwrap();

        delete_memory_group(db.clone(), group.id.clone()).expect("delete_memory_group should succeed");

        let groups = list_memory_groups(db.clone()).unwrap();
        assert!(!groups.iter().any(|g| g.id == group.id), "the group itself must be gone");

        // The file belongs to the project, not the group — it must survive.
        let still_there = get_memory_file(db, file.id).expect("the memory file must not be deleted along with the group");
        assert_eq!(still_there.project_id, proj1);
    }

    #[test]
    fn delete_memory_file_removes_it_and_drops_it_from_group_listings() {
        let (app, _acc1, _acc2, proj1) = setup_test_app();
        let db = app.state::<DbConnection>();
        let group = create_memory_group(db.clone(), "Personal projects".to_string()).unwrap();
        set_group_membership(db.clone(), group.id.clone(), proj1.clone(), true).unwrap();
        let file = create_memory_file(db.clone(), proj1, "conventions.md".to_string(), "# hi".to_string()).unwrap();

        delete_memory_file(db.clone(), file.id.clone()).expect("delete_memory_file should succeed");

        assert!(get_memory_file(db.clone(), file.id.clone()).is_err(), "the file row must be gone");
        let files = list_group_memory_files(db, group.id).unwrap();
        assert!(!files.iter().any(|f| f.id == file.id), "the deleted file must not linger in group listings");
    }

    #[test]
    fn list_discoverable_memory_files_finds_real_md_recursively_and_skips_noise_dirs() {
        let (app, _acc1, _acc2, _proj1) = setup_test_app();
        let db = app.state::<DbConnection>();

        let project_dir = std::env::temp_dir().join(format!("hangar-test-discover-md-{}", Uuid::new_v4()));
        std::fs::create_dir_all(project_dir.join("docs")).unwrap();
        std::fs::create_dir_all(project_dir.join("node_modules/some-dep")).unwrap();
        std::fs::write(project_dir.join("README.md"), "# hi").unwrap();
        std::fs::write(project_dir.join("docs/architecture.md"), "# arch").unwrap();
        std::fs::write(project_dir.join("node_modules/some-dep/CHANGELOG.md"), "noise").unwrap();
        std::fs::write(project_dir.join("not-markdown.txt"), "ignore me").unwrap();

        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO projects (id, name, path, created_at) VALUES ('proj-md','MdProject',?1,'now')",
                params![project_dir.to_string_lossy().to_string()],
            )
            .unwrap();
        }

        let found = list_discoverable_memory_files(db.clone()).unwrap();
        let paths: Vec<&str> = found.iter().filter(|f| f.project_id == "proj-md").map(|f| f.relative_path.as_str()).collect();
        assert!(paths.contains(&"README.md"));
        assert!(paths.contains(&"docs/architecture.md"));
        assert!(!paths.iter().any(|p| p.contains("node_modules")), "node_modules must be skipped, same as COPY_SKIP_DIRS");
        assert_eq!(paths.len(), 2, "the .txt file must not show up");

        let imported = import_discovered_memory_file(db.clone(), "proj-md".to_string(), "docs/architecture.md".to_string())
            .expect("importing a discovered file should succeed");
        assert_eq!(imported.content_md, "# arch");
        assert_eq!(imported.filename, "docs/architecture.md");

        let after = list_discoverable_memory_files(db).unwrap();
        assert!(
            !after.iter().any(|f| f.project_id == "proj-md" && f.relative_path == "docs/architecture.md"),
            "an imported file must no longer be listed as discoverable"
        );

        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn trigger_review_fails_clearly_when_reviewer_disabled() {
        let (app, _acc1, _acc2, _proj1) = setup_test_app();
        let err = trigger_review_blocking(app.handle().clone(), "sess1".to_string()).unwrap_err();
        assert!(err.contains("disabled"));
    }

    #[test]
    fn trigger_review_fails_clearly_when_no_reviewer_account_configured() {
        let (app, _acc1, _acc2, _proj1) = setup_test_app();
        let db = app.state::<DbConnection>();
        set_app_settings(db.clone(), AppSettings { reviewer_enabled: true, reviewer_account_id: None, ..Default::default() }).unwrap();
        let err = trigger_review_blocking(app.handle().clone(), "sess1".to_string()).unwrap_err();
        assert!(err.contains("no reviewer account"));
    }
}
