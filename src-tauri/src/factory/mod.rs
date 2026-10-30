//! The Factory: projects whose tasks an automated line takes from order to
//! delivery, every decision checked against the project's brain.
//!
//! F0 is the ground floor — which projects joined, the wizard's first step
//! (folder, repo, task provider, delivery, mode) and nothing that runs yet.

pub mod brain;
pub mod setup;
pub mod store;
pub mod team;

use std::path::Path;

use rusqlite::params;
use serde::Deserialize;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::db::DbConnection;
use setup::RepoStatus;
use store::FactoryProject;

#[tauri::command]
pub fn factory_list_projects(db: State<DbConnection>) -> Result<Vec<FactoryProject>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    store::list(&conn)
}

#[tauri::command]
pub fn factory_get_project(db: State<DbConnection>, project_id: String) -> Result<FactoryProject, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    store::get(&conn, &project_id)
}

/// Where the new Factory project lives: a folder that's already there, or one
/// to create (`parent/name`) with its repo.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NewFolder {
    Existing { path: String },
    New { parent: String, name: String },
}

/// Joins a folder to the Factory. A new folder gets `main` with a first commit
/// and `dev`; an existing one is taken as it is (the wizard offers to init it
/// when it isn't a repo). With a remote, delivery defaults to a merge request;
/// without one, to a local merge.
#[tauri::command]
pub async fn factory_create_project<R: Runtime>(app: AppHandle<R>, folder: NewFolder) -> Result<FactoryProject, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (path, fresh) = match folder {
            NewFolder::Existing { path } => (setup::expand_home(path.trim()).to_string_lossy().to_string(), false),
            // A folder that didn't exist has no project identity yet: the
            // first commit is signed with the global one.
            NewFolder::New { parent, name } => (setup::create_repo_folder(&parent, &name)?.to_string_lossy().to_string(), true),
        };
        let project = crate::commands::projects::ensure_project_for_path(db.clone(), path)?;
        let has_remote = !fresh && setup::repo_status(Path::new(&project.path)).remote_url.is_some();
        let delivery = if has_remote { "mr" } else { "local" };
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let joined = store::add(&conn, &project.id, delivery)?;
        Ok(prefill_provider(&conn, joined))
    })
    .await?
}

/// A project already wired to ClickUp or GitHub in Hangar starts with that
/// source chosen: it's where its tasks already are.
fn prefill_provider(conn: &rusqlite::Connection, project: FactoryProject) -> FactoryProject {
    if project.provider_kind != "none" {
        return project;
    }
    let link: Option<(String, String, String)> = conn
        .query_row(
            "SELECT l.source_id, s.kind, l.scope_id FROM task_source_links l JOIN task_sources s ON s.id = l.source_id
              WHERE l.project_id = ?1 AND l.scope_id IS NOT NULL AND s.kind IN ('clickup', 'github') ORDER BY s.created_at LIMIT 1",
            params![project.project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok();
    let Some((source, kind, scope)) = link else { return project };
    let label = (kind == "github").then(|| scope.clone());
    let wired = FactoryProject { provider_kind: kind, provider_source_id: Some(source), provider_scope_id: Some(scope), provider_scope_label: label, ..project.clone() };
    store::update(conn, &wired).unwrap_or(project)
}

/// Saves the wizard. A chosen provider is also linked to the project in Hangar
/// (keeping any branch template already there), so the task board and the
/// Factory read the same list.
#[tauri::command]
pub fn factory_update_project(db: State<DbConnection>, project: FactoryProject) -> Result<FactoryProject, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let saved = store::update(&conn, &project)?;
    if let (Some(source), Some(scope)) = (&saved.provider_source_id, &saved.provider_scope_id) {
        conn.execute(
            "INSERT INTO task_source_links (project_id, source_id, scope_id) VALUES (?1, ?2, ?3)
             ON CONFLICT(project_id, source_id) DO UPDATE SET scope_id = excluded.scope_id",
            params![saved.project_id, source, scope],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(saved)
}

#[tauri::command]
pub async fn factory_repo_status(path: String) -> Result<RepoStatus, String> {
    crate::blocking::run_blocking(move || setup::repo_status(&setup::expand_home(path.trim()))).await
}

/// `git init` for a Factory project whose folder wasn't a repo yet.
#[tauri::command]
pub async fn factory_init_repo<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<RepoStatus, String> {
    crate::blocking::run_blocking(move || {
        let path = {
            let db = app.state::<DbConnection>();
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            store::get(&conn, &project_id)?.project_path
        };
        let dir = Path::new(&path);
        setup::init_repo(dir, &crate::git_identity::git_env_for_project(&app, &project_id))?;
        Ok(setup::repo_status(dir))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_already_wired_to_clickup_starts_with_that_list() {
        let conn = store::tests::conn();
        conn.execute_batch(
            "CREATE TABLE task_sources (id TEXT PRIMARY KEY, kind TEXT, label TEXT, created_at TEXT);
             CREATE TABLE task_source_links (project_id TEXT, source_id TEXT, scope_id TEXT, branch_template TEXT, PRIMARY KEY (project_id, source_id));
             INSERT INTO task_sources VALUES ('s-jira', 'jira', 'Jira', '1'), ('s-cu', 'clickup', 'ClickUp', '2');
             INSERT INTO task_source_links VALUES ('p1', 's-jira', 'BOARD', NULL), ('p1', 's-cu', '901', 'CU-{id}');",
        )
        .unwrap();
        let joined = prefill_provider(&conn, store::add(&conn, "p1", "local").unwrap());
        assert_eq!((joined.provider_kind.as_str(), joined.provider_scope_id.as_deref()), ("clickup", Some("901")));
    }

    #[test]
    fn a_project_with_no_links_keeps_its_tasks_in_hangar() {
        let conn = store::tests::conn();
        conn.execute_batch(
            "CREATE TABLE task_sources (id TEXT PRIMARY KEY, kind TEXT, label TEXT, created_at TEXT);
             CREATE TABLE task_source_links (project_id TEXT, source_id TEXT, scope_id TEXT, branch_template TEXT, PRIMARY KEY (project_id, source_id));",
        )
        .unwrap();
        let joined = prefill_provider(&conn, store::add(&conn, "p1", "local").unwrap());
        assert_eq!(joined.provider_kind, "none");
    }
}
