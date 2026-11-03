use crate::commands::sessions::{cleanup_worktree_rows, WorktreeCleanupRow};
use crate::db::DbConnection;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub default_setup_command: Option<String>,
    /// Taken off the terminal pickers' recents; see `set_project_hidden`.
    pub hidden: bool,
    pub created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewProject {
    pub name: String,
    pub path: String,
}

fn row_to_project(row: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        path: row.get("path")?,
        default_setup_command: row.get("default_setup_command")?,
        hidden: row.get::<_, i64>("hidden_in_pickers")? != 0,
        created_at: row.get("created_at")?,
    })
}

#[tauri::command]
pub fn list_projects(db: State<DbConnection>) -> Result<Vec<Project>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM projects ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_project)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Reuses an existing project row when `path` is already tracked, instead of
/// creating a duplicate — every "New session" against an already-known
/// directory used to insert a fresh row (often named something quick and
/// forgettable like "Asdasd"), leaving a pile of look-alike duplicates with
/// no way to tell them apart or clean them up.
#[tauri::command]
pub fn create_project(db: State<DbConnection>, project: NewProject) -> Result<Project, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    if let Ok(existing) = conn.query_row("SELECT * FROM projects WHERE path = ?1", params![project.path], row_to_project) {
        return Ok(existing);
    }

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, project.name, project.path, now],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM projects WHERE id = ?1", params![id], row_to_project)
        .map_err(|e| e.to_string())
}

/// Get-or-create a project straight from a directory path — what "Explore a
/// folder…" in the terminal picker calls after the macOS folder dialog hands
/// back a path. Derives the name from the folder's basename so the user never
/// has to name a project just to open a terminal in it; an already-tracked
/// path reuses its row (same anti-duplication rule as `create_project`).
#[tauri::command]
pub fn ensure_project_for_path(db: State<DbConnection>, path: String) -> Result<Project, String> {
    let trimmed = path.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("no folder chosen".to_string());
    }
    if !std::path::Path::new(trimmed).is_dir() {
        return Err(format!("{trimmed} isn't a directory"));
    }

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if let Ok(existing) = conn.query_row("SELECT * FROM projects WHERE path = ?1", params![trimmed], row_to_project) {
        if !existing.hidden {
            return Ok(existing);
        }
        // Choosing a folder the user had taken off the recents is asking for
        // it back.
        conn.execute("UPDATE projects SET hidden_in_pickers = 0 WHERE id = ?1", params![existing.id])
            .map_err(|e| e.to_string())?;
        return Ok(Project { hidden: false, ..existing });
    }

    let name = std::path::Path::new(trimmed)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or(trimmed)
        .to_string();

    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO projects (id, name, path, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![id, name, trimmed, now],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM projects WHERE id = ?1", params![id], row_to_project)
        .map_err(|e| e.to_string())
}

/// The only project field worth editing after creation — changing `path`
/// isn't offered: existing worktrees under `.hangar/worktrees` (or the
/// pre-rename `.fleet/worktrees`) inside the old
/// path would silently orphan themselves.
#[tauri::command]
pub fn update_project(db: State<DbConnection>, id: String, name: String) -> Result<Project, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let name = if name.trim().is_empty() { "untitled project".to_string() } else { name.trim().to_string() };
    conn.execute("UPDATE projects SET name = ?2 WHERE id = ?1", params![id, name]).map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM projects WHERE id = ?1", params![id], row_to_project).map_err(|e| e.to_string())
}

/// Takes a project off (or puts it back on) the terminal pickers' "recent
/// projects" list. Deliberately not `delete_project`: hiding a folder from a
/// list must not cost its sessions, history or worktrees.
#[tauri::command]
pub fn set_project_hidden(db: State<DbConnection>, id: String, hidden: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let changed = conn
        .execute("UPDATE projects SET hidden_in_pickers = ?2 WHERE id = ?1", params![id, hidden as i64])
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("project not found".to_string());
    }
    Ok(())
}

/// Same shape as `accounts::delete_account`: clean up any worktrees on disk
/// first (SQL cascades can't touch the filesystem), then let `ON DELETE
/// CASCADE` — already on every table that references `project_id`
/// (`sessions`, `project_allowed_accounts`, `rotation_policies`,
/// `rotation_events`, `memory_group_projects`, `memory_files`,
/// `agent_nodes`, and everything `sessions` itself cascades to) — take care
/// of the rest in one statement.
#[tauri::command]
pub fn delete_project(db: State<DbConnection>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    delete_project_rows(&conn, &id)
}

/// `delete_project` without the command around it: its sessions' isolated
/// worktrees off the disk, then the row and everything that cascades from it.
pub(crate) fn delete_project_rows(conn: &rusqlite::Connection, id: &str) -> Result<(), String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.worktree_path, s.worktree_branch, s.worktree_kind, p.path
             FROM sessions s
             JOIN projects p ON p.id = s.project_id
             WHERE s.project_id = ?1 AND s.worktree_path IS NOT NULL",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![id], |row| {
            Ok(WorktreeCleanupRow {
                worktree_path: row.get(0)?,
                worktree_branch: row.get(1)?,
                worktree_kind: row.get(2)?,
                project_path: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let rows: Vec<_> = rows.filter_map(Result::ok).collect();
    cleanup_worktree_rows(&rows);
    drop(stmt);

    conn.execute("DELETE FROM projects WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex as StdMutex;
    use tauri::Manager;

    fn setup_test_app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        app.manage(crate::db::DbConnection(StdMutex::new(conn)));
        app
    }

    #[test]
    fn create_project_reuses_an_existing_row_for_the_same_path() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();

        let first = create_project(db.clone(), NewProject { name: "Asdasd".to_string(), path: "/tmp/real-project".to_string() }).unwrap();
        let second = create_project(db.clone(), NewProject { name: "Asdasd again".to_string(), path: "/tmp/real-project".to_string() }).unwrap();

        assert_eq!(first.id, second.id, "the same path must resolve to the same project, not a duplicate");
        assert_eq!(second.name, "Asdasd", "reusing the row must not silently rename it");

        let all = list_projects(db).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn ensure_project_for_path_derives_the_name_and_never_duplicates() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();

        // A real directory on the machine — the command validates it exists.
        let dir = std::env::temp_dir().join(format!("hangar-test-project-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();

        let created = ensure_project_for_path(db.clone(), path.clone()).unwrap();
        assert_eq!(
            created.name,
            dir.file_name().unwrap().to_string_lossy(),
            "the name must come from the folder's basename, so exploring a folder never asks the user to name it"
        );

        // A trailing slash is the same directory, and must reuse the row.
        let again = ensure_project_for_path(db.clone(), format!("{path}/")).unwrap();
        assert_eq!(again.id, created.id, "the same path (even with a trailing slash) must not create a second project");
        assert_eq!(list_projects(db).unwrap().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_project_for_path_rejects_a_path_that_isnt_a_directory() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();
        assert!(ensure_project_for_path(db.clone(), "/definitely/not/here".to_string()).is_err());
        assert!(ensure_project_for_path(db, "   ".to_string()).is_err());
    }

    #[test]
    fn update_project_renames_and_defaults_empty_name() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();
        let created = create_project(db.clone(), NewProject { name: "old".to_string(), path: "/tmp/p1".to_string() }).unwrap();

        let renamed = update_project(db.clone(), created.id.clone(), "  PropFirms  ".to_string()).unwrap();
        assert_eq!(renamed.name, "PropFirms");

        let cleared = update_project(db, created.id, "   ".to_string()).unwrap();
        assert_eq!(cleared.name, "untitled project");
    }

    #[test]
    fn set_project_hidden_only_hides_the_row() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();
        let project = create_project(db.clone(), NewProject { name: "noisy".to_string(), path: "/tmp/noisy".to_string() }).unwrap();
        assert!(!project.hidden, "a new project must show up in the pickers");

        set_project_hidden(db.clone(), project.id.clone(), true).unwrap();
        let all = list_projects(db.clone()).unwrap();
        assert_eq!(all.len(), 1, "hiding must keep the row, unlike delete_project");
        assert!(all[0].hidden);

        set_project_hidden(db.clone(), project.id.clone(), false).unwrap();
        assert!(!list_projects(db.clone()).unwrap()[0].hidden);

        assert!(set_project_hidden(db, "missing".to_string(), true).is_err());
    }

    #[test]
    fn ensure_project_for_path_brings_a_hidden_project_back() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();

        let dir = std::env::temp_dir().join(format!("hangar-test-hidden-project-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_string_lossy().to_string();

        let created = ensure_project_for_path(db.clone(), path.clone()).unwrap();
        set_project_hidden(db.clone(), created.id.clone(), true).unwrap();

        let again = ensure_project_for_path(db.clone(), path).unwrap();
        assert_eq!(again.id, created.id, "choosing the folder again must reuse the row, not duplicate it");
        assert!(!again.hidden, "choosing the folder again must bring it back to the list");
        assert!(!list_projects(db).unwrap()[0].hidden, "and the un-hide must be persisted");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Real end-to-end cascade: a project with a session, a copy-kind
    /// worktree on disk, a rotation allowlist row, and a memory group
    /// membership + file — deleting the project must take all of it with
    /// it, including the directory on disk (no git needed for a "copy"
    /// worktree — cleanup_worktree_rows just removes it directly).
    #[test]
    fn delete_project_cascades_db_rows_and_removes_worktree_from_disk() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();

        let project = create_project(db.clone(), NewProject { name: "doomed".to_string(), path: "/tmp/doomed-project-src".to_string() }).unwrap();
        let worktree_dir = std::env::temp_dir().join(format!("hangar-test-delete-project-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&worktree_dir).unwrap();
        assert!(worktree_dir.exists());

        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
                 VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO sessions (id, name, project_id, worktree_path, worktree_branch, worktree_kind, account_id, model, effort, mode, status, started_at)
                 VALUES ('sess1','test',?1,?2,NULL,'copy','acc1','sonnet','high','ask_edits','waiting','now')",
                params![project.id, worktree_dir.to_string_lossy().to_string()],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO project_allowed_accounts (project_id, account_id, allowed) VALUES (?1, 'acc1', 1)",
                params![project.id],
            )
            .unwrap();
            conn.execute("INSERT INTO memory_groups (id, name, created_at) VALUES ('g1', 'Group', 'now')", []).unwrap();
            conn.execute("INSERT INTO memory_group_projects (group_id, project_id) VALUES ('g1', ?1)", params![project.id]).unwrap();
            conn.execute(
                "INSERT INTO memory_files (id, project_id, filename, content_md, updated_at) VALUES ('f1', ?1, 'notes.md', 'hi', 'now')",
                params![project.id],
            )
            .unwrap();
        }

        delete_project(db.clone(), project.id.clone()).expect("delete_project should succeed");

        assert!(!worktree_dir.exists(), "the worktree directory must be removed from disk");
        let conn = db.0.lock().unwrap();
        let project_count: i64 = conn.query_row("SELECT COUNT(*) FROM projects WHERE id = ?1", params![project.id], |r| r.get(0)).unwrap();
        let session_count: i64 = conn.query_row("SELECT COUNT(*) FROM sessions WHERE project_id = ?1", params![project.id], |r| r.get(0)).unwrap();
        let allowed_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM project_allowed_accounts WHERE project_id = ?1", params![project.id], |r| r.get(0)).unwrap();
        let group_membership_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM memory_group_projects WHERE project_id = ?1", params![project.id], |r| r.get(0)).unwrap();
        let memory_file_count: i64 = conn.query_row("SELECT COUNT(*) FROM memory_files WHERE project_id = ?1", params![project.id], |r| r.get(0)).unwrap();
        assert_eq!(project_count, 0);
        assert_eq!(session_count, 0, "sessions must cascade");
        assert_eq!(allowed_count, 0, "rotation allowlist rows must cascade");
        assert_eq!(group_membership_count, 0, "memory group membership must cascade");
        assert_eq!(memory_file_count, 0, "memory files must cascade");
    }
}
