//! The Factory's rows: one per project that joined it, holding what the setup
//! wizard decided — where tasks come from, how an order is delivered, the mode
//! and how far the setup went.

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub const PROVIDERS: [&str; 3] = ["clickup", "github", "none"];
pub const DELIVERIES: [&str; 2] = ["mr", "local"];
pub const MODES: [&str; 3] = ["supervised", "delegated", "autonomous"];
/// Project, Team, Brain, Tasks, Play — and 6 once ▶ Play was pressed and
/// the project shows the line at work.
pub const LAST_STEP: i64 = 6;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FactoryProject {
    pub project_id: String,
    /// From `projects`; read-only here.
    #[serde(default)]
    pub project_name: String,
    #[serde(default)]
    pub project_path: String,
    /// `clickup`, `github` or `none` (the tasks live in Hangar).
    pub provider_kind: String,
    /// The task source (`task_sources.id`) for ClickUp or GitHub.
    #[serde(default)]
    pub provider_source_id: Option<String>,
    /// Only this project's tasks: a ClickUp list id, or a GitHub `owner/repo`.
    #[serde(default)]
    pub provider_scope_id: Option<String>,
    /// What the scope is called on screen (`Space / Folder / List`).
    #[serde(default)]
    pub provider_scope_label: Option<String>,
    /// `mr` or `local` (merged into the local base branch, never pushed).
    pub delivery: String,
    pub mode: String,
    /// How to start the project's app, for the UI and UX reviewers.
    #[serde(default)]
    pub app_start_cmd: Option<String>,
    #[serde(default)]
    pub app_url: Option<String>,
    /// The wizard step the user is on, 1–5; 6 once it's running.
    pub setup_step: i64,
    /// `stopped`, `running` or `paused`.
    #[serde(default = "stopped")]
    pub loop_state: String,
    /// The sealed brain version new orders freeze; `None` until one is sealed.
    #[serde(default)]
    pub brain_version: Option<i64>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

fn stopped() -> String {
    "stopped".to_string()
}

const SELECT: &str = "SELECT f.project_id, p.name, p.path, f.provider_kind, f.provider_source_id, f.provider_scope_id, f.provider_scope_label,
        f.delivery, f.mode, f.app_start_cmd, f.app_url, f.setup_step, f.loop_state, f.created_at, f.updated_at, f.brain_version
   FROM factory_projects f JOIN projects p ON p.id = f.project_id";

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<FactoryProject> {
    Ok(FactoryProject {
        project_id: r.get(0)?,
        project_name: r.get(1)?,
        project_path: r.get(2)?,
        provider_kind: r.get(3)?,
        provider_source_id: r.get(4)?,
        provider_scope_id: r.get(5)?,
        provider_scope_label: r.get(6)?,
        delivery: r.get(7)?,
        mode: r.get(8)?,
        app_start_cmd: r.get(9)?,
        app_url: r.get(10)?,
        setup_step: r.get(11)?,
        loop_state: r.get(12)?,
        created_at: r.get(13)?,
        updated_at: r.get(14)?,
        brain_version: r.get(15)?,
    })
}

pub fn list(conn: &Connection) -> Result<Vec<FactoryProject>, String> {
    let mut stmt = conn.prepare(&format!("{SELECT} ORDER BY f.created_at")).map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], from_row).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

pub fn get(conn: &Connection, project_id: &str) -> Result<FactoryProject, String> {
    conn.query_row(&format!("{SELECT} WHERE f.project_id = ?1"), params![project_id], from_row)
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "this project isn't in the Factory".to_string())
}

/// Adds a project with its first choices. Joining twice keeps the first row.
pub fn add(conn: &Connection, project_id: &str, delivery: &str) -> Result<FactoryProject, String> {
    let delivery = if DELIVERIES.contains(&delivery) { delivery } else { "local" };
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT OR IGNORE INTO factory_projects (project_id, delivery, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
        params![project_id, delivery, now],
    )
    .map_err(|e| e.to_string())?;
    get(conn, project_id)
}

fn blank(v: &Option<String>) -> Option<String> {
    v.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Saves what the wizard says, refusing values the rest of the Factory can't
/// act on. A provider of `none` drops its source and scope.
pub fn update(conn: &Connection, p: &FactoryProject) -> Result<FactoryProject, String> {
    if !PROVIDERS.contains(&p.provider_kind.as_str()) {
        return Err(format!("unknown task provider `{}`", p.provider_kind));
    }
    if !DELIVERIES.contains(&p.delivery.as_str()) {
        return Err(format!("unknown delivery `{}`", p.delivery));
    }
    if !MODES.contains(&p.mode.as_str()) {
        return Err(format!("unknown mode `{}`", p.mode));
    }
    let none = p.provider_kind == "none";
    let (source, scope, label) = if none { (None, None, None) } else { (blank(&p.provider_source_id), blank(&p.provider_scope_id), blank(&p.provider_scope_label)) };
    if !none && (source.is_none() || scope.is_none()) {
        return Err("choose the account and the list (or repo) the project's tasks come from".to_string());
    }
    let changed = conn
        .execute(
            "UPDATE factory_projects SET provider_kind = ?2, provider_source_id = ?3, provider_scope_id = ?4, provider_scope_label = ?5,
               delivery = ?6, mode = ?7, app_start_cmd = ?8, app_url = ?9, setup_step = ?10, updated_at = ?11
             WHERE project_id = ?1",
            params![
                p.project_id,
                p.provider_kind,
                source,
                scope,
                label,
                p.delivery,
                p.mode,
                blank(&p.app_start_cmd),
                blank(&p.app_url),
                p.setup_step.clamp(1, LAST_STEP),
                Utc::now().to_rfc3339()
            ],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("this project isn't in the Factory".to_string());
    }
    get(conn, &p.project_id)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON; CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT, path TEXT);").unwrap();
        for ddl in crate::db::FACTORY_DDLS {
            conn.execute_batch(ddl).unwrap();
        }
        conn.execute_batch("INSERT INTO projects VALUES ('p1', 'citas-vet', '/repos/citas-vet');").unwrap();
        conn
    }

    #[test]
    fn a_project_joins_once_with_safe_defaults() {
        let c = conn();
        let p = add(&c, "p1", "mr").unwrap();
        assert_eq!((p.project_name.as_str(), p.provider_kind.as_str(), p.delivery.as_str(), p.mode.as_str()), ("citas-vet", "none", "mr", "supervised"));
        assert_eq!((p.setup_step, p.loop_state.as_str()), (1, "stopped"));
        add(&c, "p1", "local").unwrap();
        assert_eq!(list(&c).unwrap().len(), 1, "joining twice keeps one row");
        assert_eq!(get(&c, "p1").unwrap().delivery, "mr", "and its first choices");
        assert!(get(&c, "nope").is_err());
    }

    #[test]
    fn the_wizard_saves_only_what_the_factory_can_act_on() {
        let c = conn();
        let mut p = add(&c, "p1", "local").unwrap();
        p.provider_kind = "clickup".into();
        assert!(update(&c, &p).is_err(), "ClickUp without a list is refused");
        p.provider_source_id = Some("src-1".into());
        p.provider_scope_id = Some("901".into());
        p.provider_scope_label = Some("Clínica / Citas Vet".into());
        p.mode = "autonomous".into();
        p.app_url = Some("  http://localhost:8000 ".into());
        p.setup_step = 9;
        let saved = update(&c, &p).unwrap();
        assert_eq!((saved.provider_scope_id.as_deref(), saved.mode.as_str()), (Some("901"), "autonomous"));
        assert_eq!((saved.app_url.as_deref(), saved.setup_step), (Some("http://localhost:8000"), LAST_STEP));

        p.provider_kind = "none".into();
        let local = update(&c, &p).unwrap();
        assert_eq!((local.provider_source_id, local.provider_scope_id), (None, None), "no provider: tasks stay in Hangar");

        for (field, value) in [("provider", "jira"), ("delivery", "push"), ("mode", "yolo")] {
            let mut bad = p.clone();
            match field {
                "provider" => bad.provider_kind = value.into(),
                "delivery" => bad.delivery = value.into(),
                _ => bad.mode = value.into(),
            }
            assert!(update(&c, &bad).is_err(), "{field} = {value} must be refused");
        }
    }

    #[test]
    fn leaving_hangar_takes_the_factory_row_with_it() {
        let c = conn();
        add(&c, "p1", "local").unwrap();
        c.execute("DELETE FROM projects WHERE id = 'p1'", []).unwrap();
        assert!(list(&c).unwrap().is_empty());
    }
}
