//! Knowledge sources: where a team keeps what it knows — a wiki, a docs site —
//! and the piece of it an agent can reach while it works.
//!
//! Shaped like `task_sources`: a row per source, a `kind` that picks the
//! adapter, the secret in the keychain and only its reference in the database.
//! Two kinds today:
//!
//! - **`outline`** — an Outline instance (cloud or self-hosted). Its own MCP
//!   server lives at `<base>/mcp`, and its REST API is what later features
//!   (search from Hangar, related docs in a task brief) will read.
//! - **`mcp`** — anything else with an MCP endpoint over HTTP. Hangar can't read
//!   it itself, but it can hand it to every agent, which is the main point.
//!
//! What every source becomes, for an agent, is an [`AgentServer`]: a name, an
//! MCP URL and a token. `crate::agent_mcp` turns those into each CLI's flags.

pub mod mcp;
pub mod outline;

use crate::db::DbConnection;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use uuid::Uuid;

/// One source, as the settings screen shows it. The token never comes back.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeSourceView {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub base_url: String,
    /// Where agents reach it — shown so "which server is this" has an answer.
    pub mcp_url: String,
    pub has_credential: bool,
    pub config: serde_json::Value,
    pub all_projects: bool,
    /// The projects it's limited to, when `all_projects` is off.
    pub project_ids: Vec<String>,
}

/// One source as an agent sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AgentServer {
    /// The MCP server's name — what an agent's tools are prefixed with, so a
    /// readable one (`team-docs`) beats an id.
    pub name: String,
    pub url: String,
    pub token: Option<String>,
}

fn mcp_url_for(kind: &str, base_url: &str) -> String {
    match kind {
        "outline" => outline::mcp_url(base_url),
        _ => base_url.to_string(),
    }
}

/// `Team docs` → `team-docs`. Only what every CLI accepts in a server
/// name, since this one string is also a TOML key for Codex.
pub(crate) fn slug(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_string();
    if out.is_empty() {
        "wiki".to_string()
    } else {
        out
    }
}

/// Whether any source is set up — read by the "what Hangar needs" row, which
/// has no database handle and must not wait on one. Kept current by
/// [`refresh_connected`] at startup and after every save or delete.
static CONNECTED: AtomicBool = AtomicBool::new(false);

/// Serializes the tests that flip [`CONNECTED`]: it is process-wide, and
/// `cargo test` runs in threads.
#[cfg(test)]
pub(crate) fn lock_connected_for_test() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn any_connected() -> bool {
    CONNECTED.load(Ordering::Relaxed)
}

pub(crate) fn refresh_connected(conn: &Connection) {
    let any = conn.query_row("SELECT EXISTS (SELECT 1 FROM knowledge_sources)", [], |r| r.get::<_, bool>(0)).unwrap_or(false);
    CONNECTED.store(any, Ordering::Relaxed);
}

/// The sources this project's agents get: every one set to "all projects",
/// plus the ones limited to a list that includes it. Ordered by when they were
/// added, and named uniquely, so two wikis labelled alike don't collide.
///
/// The token is read here from the keychain (cached per launch); a source whose
/// secret has gone is still handed over without one rather than dropped, so
/// the agent's error names the server instead of it silently vanishing.
pub(crate) fn servers_for_project(conn: &Connection, project_id: &str) -> Vec<AgentServer> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT s.kind, s.label, s.base_url, s.credential_ref FROM knowledge_sources s
          WHERE s.all_projects = 1
             OR EXISTS (SELECT 1 FROM knowledge_source_links l WHERE l.source_id = s.id AND l.project_id = ?1)
          ORDER BY s.created_at",
    ) else {
        return Vec::new();
    };
    let Ok(rows) = stmt.query_map(params![project_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?))
    }) else {
        return Vec::new();
    };
    let mut servers: Vec<AgentServer> = Vec::new();
    for (kind, label, base_url, credential_ref) in rows.filter_map(Result::ok) {
        let base = slug(&label);
        let mut name = base.clone();
        let mut n = 2;
        while servers.iter().any(|s| s.name == name) {
            name = format!("{base}-{n}");
            n += 1;
        }
        let token = credential_ref.and_then(|r| crate::credential_vault::get(&r).ok());
        servers.push(AgentServer { name, url: mcp_url_for(&kind, &base_url), token });
    }
    servers
}

// ---------------------------------------------------------------- commands

fn project_ids(conn: &Connection, source_id: &str) -> Vec<String> {
    let Ok(mut stmt) = conn.prepare("SELECT project_id FROM knowledge_source_links WHERE source_id = ?1") else { return Vec::new() };
    let Ok(rows) = stmt.query_map(params![source_id], |r| r.get::<_, String>(0)) else { return Vec::new() };
    rows.filter_map(Result::ok).collect()
}

fn all_sources(conn: &Connection) -> Result<Vec<KnowledgeSourceView>, String> {
    let mut stmt = conn
        .prepare("SELECT id, kind, label, base_url, credential_ref, config_json, all_projects FROM knowledge_sources ORDER BY created_at")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, String, String, Option<String>, String, bool)> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows
        .into_iter()
        .map(|(id, kind, label, base_url, credential_ref, config_json, all_projects)| KnowledgeSourceView {
            project_ids: project_ids(conn, &id),
            mcp_url: mcp_url_for(&kind, &base_url),
            // Asked of the keychain, not of the row — the same rule as task
            // sources: a row claiming a token the keychain lost sends you
            // looking anywhere but at the token.
            has_credential: credential_ref.as_deref().is_some_and(crate::credential_vault::has),
            config: serde_json::from_str(&config_json).unwrap_or(serde_json::Value::Object(Default::default())),
            id,
            kind,
            label,
            base_url,
            all_projects,
        })
        .collect())
}

#[tauri::command]
pub fn list_knowledge_sources(db: tauri::State<DbConnection>) -> Result<Vec<KnowledgeSourceView>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    all_sources(&conn)
}

/// Adds or updates a source, checking the address and the token against the
/// tool **before** storing either: a wrong key would otherwise surface as an
/// agent saying "the wiki tool failed", far from the form that can fix it.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn save_knowledge_source<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: Option<String>,
    kind: String,
    label: String,
    base_url: String,
    token: Option<String>,
    all_projects: bool,
    project_ids: Vec<String>,
) -> Result<KnowledgeSourceView, String> {
    crate::blocking::run_blocking(move || {
        use tauri::Manager;
        let db = app.state::<DbConnection>();

        let existing_ref: Option<String> = match &id {
            Some(id) => {
                let conn = db.0.lock().map_err(|e| e.to_string())?;
                conn.query_row("SELECT credential_ref FROM knowledge_sources WHERE id = ?1", params![id], |r| r.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?
                    .flatten()
            }
            None => None,
        };
        // Blank means "keep what's stored": the token never comes back to the
        // form, so blank is the normal state of an edit.
        let typed = token.as_deref().map(str::trim).filter(|t| !t.is_empty()).map(str::to_string);
        let token = match typed {
            Some(t) => Some(t),
            None => existing_ref.as_deref().and_then(|r| crate::credential_vault::get(r).ok()),
        };

        let (base_url, config) = match kind.as_str() {
            "outline" => {
                let base = outline::normalize_base(&base_url)?;
                let token = token.as_deref().ok_or("Outline needs an API key — Outline → Settings → API & Apps → New API key")?;
                let who = outline::verify(&base, token)?;
                (base, serde_json::json!({ "user": who.user, "team": who.team }))
            }
            "mcp" => {
                let url = mcp::normalize_url(&base_url)?;
                let server = mcp::verify(&url, token.as_deref())?;
                (url, serde_json::json!({ "server": server }))
            }
            other => return Err(format!("unknown knowledge source: {other}")),
        };

        let credential_ref = match &token {
            Some(token) => {
                let reference = existing_ref.clone().unwrap_or_else(|| format!("knowledge-source-{}", Uuid::new_v4()));
                crate::credential_vault::store(&reference, token)?;
                Some(reference)
            }
            None => None,
        };

        let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let label = if label.trim().is_empty() { config.get("team").or(config.get("server")).and_then(|v| v.as_str()).unwrap_or("Wiki").to_string() } else { label.trim().to_string() };
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO knowledge_sources (id, kind, label, base_url, credential_ref, config_json, all_projects, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET label = ?3, base_url = ?4, credential_ref = ?5, config_json = ?6, all_projects = ?7",
            params![id, kind, label, base_url, credential_ref, config.to_string(), all_projects, Utc::now().to_rfc3339()],
        )
        .map_err(|e| e.to_string())?;
        set_links(&conn, &id, &project_ids)?;
        refresh_connected(&conn);
        all_sources(&conn)?.into_iter().find(|s| s.id == id).ok_or_else(|| "the source vanished as it was saved".to_string())
    })
    .await?
}

fn set_links(conn: &Connection, source_id: &str, project_ids: &[String]) -> Result<(), String> {
    conn.execute("DELETE FROM knowledge_source_links WHERE source_id = ?1", params![source_id]).map_err(|e| e.to_string())?;
    for project_id in project_ids {
        conn.execute(
            "INSERT OR IGNORE INTO knowledge_source_links (project_id, source_id) VALUES (?1, ?2)",
            params![project_id, source_id],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn delete_knowledge_source(db: tauri::State<DbConnection>, source_id: String) -> Result<(), String> {
    let credential_ref: Option<String> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let reference = conn
            .query_row("SELECT credential_ref FROM knowledge_sources WHERE id = ?1", params![source_id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        conn.execute("DELETE FROM knowledge_sources WHERE id = ?1", params![source_id]).map_err(|e| e.to_string())?;
        refresh_connected(&conn);
        reference
    };
    // After the row, and ignoring the result — the same order as task sources:
    // an orphaned keychain entry is inert, a row pointing at nothing is not.
    if let Some(reference) = credential_ref {
        let _ = crate::credential_vault::delete(&reference);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY);").unwrap();
        conn.execute_batch(crate::db::KNOWLEDGE_SOURCES_DDL).unwrap();
        conn.execute_batch("INSERT INTO projects (id) VALUES ('p1'), ('p2');").unwrap();
        conn
    }

    fn add(conn: &Connection, id: &str, kind: &str, label: &str, base: &str, all: bool, at: &str) {
        conn.execute(
            "INSERT INTO knowledge_sources (id, kind, label, base_url, credential_ref, all_projects, created_at) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6)",
            params![id, kind, label, base, all, at],
        )
        .unwrap();
    }

    #[test]
    fn a_label_becomes_a_name_every_cli_accepts() {
        assert_eq!(slug("Team docs"), "team-docs");
        assert_eq!(slug("  Wiki (prod) — 2 "), "wiki-prod-2");
        assert_eq!(slug("¡¿!"), "wiki");
    }

    #[test]
    fn a_project_gets_the_sources_for_everyone_and_the_ones_meant_for_it() {
        let conn = conn();
        add(&conn, "a", "outline", "Docs", "https://docs.acme.com", true, "1");
        add(&conn, "b", "mcp", "Docs", "https://mcp.notion.com/mcp", false, "2");
        add(&conn, "c", "mcp", "Other", "https://x.example/mcp", false, "3");
        set_links(&conn, "b", &["p1".to_string()]).unwrap();

        let p1 = servers_for_project(&conn, "p1");
        assert_eq!(
            p1.iter().map(|s| (s.name.as_str(), s.url.as_str())).collect::<Vec<_>>(),
            vec![("docs", "https://docs.acme.com/mcp"), ("docs-2", "https://mcp.notion.com/mcp")],
            "Outline's MCP is under /mcp; a generic one is its URL; two alike labels don't collide"
        );
        assert_eq!(servers_for_project(&conn, "p2").len(), 1, "p2 only gets the one for everyone");
    }

    #[test]
    fn the_needs_row_knows_whether_anything_is_connected() {
        let _guard = lock_connected_for_test();
        let conn = conn();
        refresh_connected(&conn);
        assert!(!any_connected());
        add(&conn, "a", "outline", "Docs", "https://docs.acme.com", true, "1");
        refresh_connected(&conn);
        assert!(any_connected());
        conn.execute("DELETE FROM knowledge_sources", []).unwrap();
        refresh_connected(&conn);
        assert!(!any_connected());
    }
}
