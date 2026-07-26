//! Hierarchical "teams" of agents (tree-canvas) — CRUD over the `agent_nodes`
//! table (already in `schema.sql`, previously unused: `role`/`prompt`/`model`/
//! `account_id`, up to 3 levels deep via `parent_id`/`depth`), plus
//! `run_agent_tree`, a deterministic bottom-up executor: a node's children
//! all run first (in parallel), and once every child has finished, its
//! output is folded into the parent's prompt as context before the parent's
//! own turn starts. This is NOT autonomous agent-to-agent delegation (no
//! agent decides to invoke another) — Hangar walks the tree and sequences the
//! turns itself, same spirit as CrewAI's deterministic "Flow" mode rather
//! than its free-form "Crew" mode. See the tree-canvas plan for why: Hangar
//! has no inter-agent message bus, and building one is a much bigger project
//! than this screen.

use crate::db::DbConnection;
use crate::models::AgentNode;
use chrono::Utc;
use rusqlite::params;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};
use uuid::Uuid;

fn row_to_agent_node(row: &rusqlite::Row) -> rusqlite::Result<AgentNode> {
    Ok(AgentNode {
        id: row.get("id")?,
        project_id: row.get("project_id")?,
        parent_id: row.get("parent_id")?,
        role: row.get("role")?,
        prompt: row.get("prompt")?,
        model: row.get("model")?,
        account_id: row.get("account_id")?,
        depth: row.get("depth")?,
        created_at: row.get("created_at")?,
    })
}

#[tauri::command]
pub fn list_agent_nodes(db: State<DbConnection>, project_id: String) -> Result<Vec<AgentNode>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT * FROM agent_nodes WHERE project_id = ?1 ORDER BY created_at ASC").map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![project_id], row_to_agent_node).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

const MAX_DEPTH: i64 = 3;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAgentNode {
    pub project_id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub prompt: String,
    pub model: String,
    pub account_id: Option<String>,
}

pub(crate) fn node_depth(conn: &rusqlite::Connection, parent_id: Option<&str>) -> Result<i64, String> {
    match parent_id {
        Some(pid) => {
            let parent_depth: i64 =
                conn.query_row("SELECT depth FROM agent_nodes WHERE id = ?1", params![pid], |r| r.get(0)).map_err(|e| format!("parent node lookup failed: {e}"))?;
            Ok(parent_depth + 1)
        }
        None => Ok(0),
    }
}

#[tauri::command]
pub fn create_agent_node(db: State<DbConnection>, input: NewAgentNode) -> Result<AgentNode, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let depth = node_depth(&conn, input.parent_id.as_deref())?;
    if depth > MAX_DEPTH {
        return Err(format!("a tree can only go {MAX_DEPTH} levels deep"));
    }
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO agent_nodes (id, project_id, parent_id, role, prompt, model, account_id, depth, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![id, input.project_id, input.parent_id, input.role, input.prompt, input.model, input.account_id, depth, now],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM agent_nodes WHERE id = ?1", params![id], row_to_agent_node).map_err(|e| e.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAgentNode {
    pub id: String,
    pub role: String,
    pub prompt: String,
    pub model: String,
    pub account_id: Option<String>,
}

#[tauri::command]
pub fn update_agent_node(db: State<DbConnection>, input: UpdateAgentNode) -> Result<AgentNode, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE agent_nodes SET role = ?2, prompt = ?3, model = ?4, account_id = ?5 WHERE id = ?1",
        params![input.id, input.role, input.prompt, input.model, input.account_id],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM agent_nodes WHERE id = ?1", params![input.id], row_to_agent_node).map_err(|e| e.to_string())
}

/// Cascades to every descendant automatically (`agent_nodes.parent_id` is
/// `ON DELETE CASCADE`) — deleting a subtree's root takes the whole branch
/// with it in one statement.
#[tauri::command]
pub fn delete_agent_node(db: State<DbConnection>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM agent_nodes WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Clone)]
struct LoadedNode {
    role: String,
    prompt: String,
    model: String,
    account_id: Option<String>,
    project_id: String,
    children: Vec<String>,
}

pub(crate) fn load_subtree(conn: &rusqlite::Connection, root_id: &str) -> Result<HashMap<String, LoadedNode>, String> {
    let root_project_id: String =
        conn.query_row("SELECT project_id FROM agent_nodes WHERE id = ?1", params![root_id], |r| r.get(0)).map_err(|e| format!("root node lookup failed: {e}"))?;

    let mut stmt = conn
        .prepare("SELECT id, project_id, parent_id, role, prompt, model, account_id FROM agent_nodes WHERE project_id = ?1")
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, Option<String>, String, String, String, Option<String>)> = stmt
        .query_map(params![root_project_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    let mut nodes: HashMap<String, LoadedNode> = HashMap::new();
    for (id, project_id, _parent_id, role, prompt, model, account_id) in &rows {
        nodes.insert(
            id.clone(),
            LoadedNode { role: role.clone(), prompt: prompt.clone(), model: model.clone(), account_id: account_id.clone(), project_id: project_id.clone(), children: Vec::new() },
        );
    }
    for (id, _project_id, parent_id, ..) in &rows {
        if let Some(pid) = parent_id {
            if let Some(parent) = nodes.get_mut(pid) {
                parent.children.push(id.clone());
            }
        }
    }
    Ok(nodes)
}

/// A real agent turn can legitimately run for many minutes — this is
/// deliberately NOT the short `process_timeout` ceiling used for `git`/`gh`
/// calls elsewhere; it just needs an outer bound so a genuinely stuck turn
/// doesn't hang this command forever.
const MAX_TURN_WAIT: Duration = Duration::from_secs(30 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub(crate) fn wait_for_turn_to_finish(db: &DbConnection, session_id: &str) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let status: String = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT status FROM sessions WHERE id = ?1", params![session_id], |r| r.get(0)).map_err(|e| e.to_string())?
        };
        // `mark_session_status` always returns a turn to "waiting" — on
        // success, on a spawn failure, and on every other error path (see
        // `turn_runner::spawn_turn`) — so "not running anymore" is the
        // signal this turn is over, not a distinct "done"/"error" status.
        if status != "running" {
            return Ok(());
        }
        if start.elapsed() > MAX_TURN_WAIT {
            return Err(format!("timed out after {}m waiting for the turn to finish", MAX_TURN_WAIT.as_secs() / 60));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// The most recent real response text from a session — what a parent node
/// reads as "what this sub-agent reported." Falls back to an explicit
/// placeholder rather than an empty string so a parent's prompt never
/// silently loses a section.
pub(crate) fn fetch_last_output(db: &DbConnection, session_id: &str) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.query_row(
        "SELECT text FROM session_blocks WHERE session_id = ?1 AND kind IN ('agent','out') ORDER BY created_at DESC LIMIT 1",
        params![session_id],
        |r| r.get(0),
    )
    .or_else(|_| Ok("(no output)".to_string()))
}

fn ensure_session_for_node(db: &DbConnection, node_id: &str, node: &LoadedNode) -> Result<String, String> {
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let existing: Option<String> =
            conn.query_row("SELECT id FROM sessions WHERE node_id = ?1 ORDER BY started_at DESC LIMIT 1", params![node_id], |r| r.get(0)).ok();
        if let Some(session_id) = existing {
            return Ok(session_id);
        }
    }
    let account_id = node.account_id.clone().ok_or_else(|| format!("the '{}' node has no account assigned — set one before running the tree", node.role))?;

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let session_id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO sessions (id, name, project_id, node_id, account_id, model, effort, mode, status, pid, provider_session_id, started_at, ended_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'high', 'ask_edits', 'waiting', NULL, NULL, ?7, NULL)",
        params![session_id, node.role, node.project_id, node_id, account_id, node.model, now],
    )
    .map_err(|e| e.to_string())?;
    Ok(session_id)
}

/// Runs one node: waits on every child first (in parallel), folds their
/// output into this node's prompt, then runs this node's own turn and
/// returns its final output for its own parent to consume in turn.
fn run_node_recursive(app: &AppHandle, nodes: &HashMap<String, LoadedNode>, node_id: &str) -> Result<String, String> {
    let node = nodes.get(node_id).ok_or_else(|| format!("node {node_id} not found"))?;

    let mut child_outputs: Vec<(String, Result<String, String>)> = Vec::new();
    if !node.children.is_empty() {
        // Children run in parallel, but only across *distinct accounts*. Two
        // siblings sharing an account would mean two concurrent `claude`
        // processes against one `CLAUDE_CONFIG_DIR` — they'd race on the same
        // on-disk credential/transcript state, and burn one account's rate
        // limit twice as fast for no parallelism gain, since the bottleneck
        // is the account and not the machine. So: one thread per account,
        // and that thread walks its own children one at a time. A child with
        // no account assigned gets its own bucket keyed by node id, since
        // `ensure_session_for_node` will reject it individually anyway.
        let mut buckets: HashMap<String, Vec<String>> = HashMap::new();
        for child_id in &node.children {
            let key = nodes
                .get(child_id)
                .and_then(|n| n.account_id.clone())
                .unwrap_or_else(|| format!("__unassigned__{child_id}"));
            buckets.entry(key).or_default().push(child_id.clone());
        }

        std::thread::scope(|scope| {
            let handles: Vec<_> = buckets
                .into_values()
                .map(|child_ids| {
                    let app = app.clone();
                    scope.spawn(move || {
                        child_ids
                            .into_iter()
                            .map(|child_id| {
                                let role = nodes.get(&child_id).map(|n| n.role.clone()).unwrap_or_default();
                                (role, run_node_recursive(&app, nodes, &child_id))
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            for handle in handles {
                match handle.join() {
                    Ok(results) => child_outputs.extend(results),
                    Err(_) => child_outputs.push((String::new(), Err("child agent thread panicked".to_string()))),
                }
            }
        });
    }

    let full_prompt = if child_outputs.is_empty() {
        node.prompt.clone()
    } else {
        let mut ctx = String::from("Here is what your sub-agents reported:\n\n");
        for (role, result) in &child_outputs {
            let text = match result {
                Ok(t) => t.clone(),
                // A failed sub-agent still gets reported up instead of
                // blocking the whole tree — the parent (often a reviewer/
                // synthesizer) is exactly the agent best placed to notice
                // and react to a sub-agent that didn't complete.
                Err(e) => format!("(this sub-agent did not complete: {e})"),
            };
            ctx.push_str(&format!("### {role}\n{text}\n\n"));
        }
        ctx.push_str(&node.prompt);
        ctx
    };

    let db = app.state::<DbConnection>();
    let session_id = ensure_session_for_node(&db, node_id, node)?;
    crate::commands::sessions::send_message(app.clone(), db.clone(), session_id.clone(), full_prompt)?;
    wait_for_turn_to_finish(&db, &session_id)?;
    fetch_last_output(&db, &session_id)
}

/// Entry point for the "▶ Run tree" button — `root_id` is any node, but
/// typically the tree's actual root (a node with no parent); running a
/// non-root node just runs that smaller subtree instead of the whole thing.
#[tauri::command]
pub async fn run_agent_tree(app: AppHandle, root_id: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || {
        let nodes = {
            let db = app.state::<DbConnection>();
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            load_subtree(&conn, &root_id)?
        };
        run_node_recursive(&app, &nodes, &root_id).map(|_| ())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex as StdMutex;
    use tauri::Manager as _;

    fn setup_test_app() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        app.manage(crate::db::DbConnection(StdMutex::new(conn)));
        app
    }

    fn seed_project(db: &DbConnection) -> String {
        let conn = db.0.lock().unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp/proj1','now')", []).unwrap();
        "proj1".to_string()
    }

    #[test]
    fn create_agent_node_computes_depth_and_rejects_beyond_max() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();
        let project_id = seed_project(&db);

        let root = create_agent_node(
            db.clone(),
            NewAgentNode { project_id: project_id.clone(), parent_id: None, role: "Lead".into(), prompt: "lead".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();
        assert_eq!(root.depth, 0);

        let child = create_agent_node(
            db.clone(),
            NewAgentNode { project_id: project_id.clone(), parent_id: Some(root.id.clone()), role: "Researcher".into(), prompt: "research".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();
        assert_eq!(child.depth, 1);

        let grandchild = create_agent_node(
            db.clone(),
            NewAgentNode { project_id: project_id.clone(), parent_id: Some(child.id.clone()), role: "Sub".into(), prompt: "sub".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();
        assert_eq!(grandchild.depth, 2);

        let great_grandchild = create_agent_node(
            db.clone(),
            NewAgentNode { project_id, parent_id: Some(grandchild.id.clone()), role: "TooDeep".into(), prompt: "x".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();
        assert_eq!(great_grandchild.depth, 3);

        let rejected = create_agent_node(
            db,
            NewAgentNode { project_id: "proj1".into(), parent_id: Some(great_grandchild.id), role: "WayTooDeep".into(), prompt: "x".into(), model: "sonnet".into(), account_id: None },
        );
        assert!(rejected.is_err(), "depth 4 must be rejected");
    }

    #[test]
    fn delete_agent_node_cascades_to_children() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();
        let project_id = seed_project(&db);

        let root = create_agent_node(
            db.clone(),
            NewAgentNode { project_id: project_id.clone(), parent_id: None, role: "Lead".into(), prompt: "".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();
        let child = create_agent_node(
            db.clone(),
            NewAgentNode { project_id, parent_id: Some(root.id.clone()), role: "Researcher".into(), prompt: "".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();

        delete_agent_node(db.clone(), root.id.clone()).unwrap();

        let remaining = list_agent_nodes(db, "proj1".to_string()).unwrap();
        assert!(remaining.is_empty(), "deleting the root must cascade to its child ({child_id})", child_id = child.id);
    }

    #[test]
    fn load_subtree_builds_the_correct_parent_child_map() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();
        let project_id = seed_project(&db);

        let root = create_agent_node(
            db.clone(),
            NewAgentNode { project_id: project_id.clone(), parent_id: None, role: "Lead".into(), prompt: "".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();
        let leaf_a = create_agent_node(
            db.clone(),
            NewAgentNode { project_id: project_id.clone(), parent_id: Some(root.id.clone()), role: "Research A".into(), prompt: "".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();
        let leaf_b = create_agent_node(
            db.clone(),
            NewAgentNode { project_id, parent_id: Some(root.id.clone()), role: "Research B".into(), prompt: "".into(), model: "sonnet".into(), account_id: None },
        )
        .unwrap();

        let conn = db.0.lock().unwrap();
        let nodes = load_subtree(&conn, &root.id).unwrap();
        drop(conn);

        assert_eq!(nodes.len(), 3);
        let mut children = nodes[&root.id].children.clone();
        children.sort();
        let mut expected = vec![leaf_a.id, leaf_b.id];
        expected.sort();
        assert_eq!(children, expected);
    }

    #[test]
    fn wait_for_turn_to_finish_returns_as_soon_as_status_leaves_running() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
                 VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
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
                 VALUES ('sess1','node session','proj1','acc1','sonnet','high','ask_edits','waiting','now')",
                [],
            )
            .unwrap();
        }
        // Already "waiting" (never went "running") — must return immediately, not time out.
        wait_for_turn_to_finish(&db, "sess1").expect("a session that's already past 'running' must resolve immediately");
    }

    #[test]
    fn fetch_last_output_reads_the_most_recent_agent_block() {
        let app = setup_test_app();
        let db = app.state::<DbConnection>();
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
                 VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
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
                 VALUES ('sess1','node session','proj1','acc1','sonnet','high','ask_edits','waiting','now')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_blocks (id, session_id, kind, text, created_at) VALUES ('b1','sess1','you','hello','2020-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_blocks (id, session_id, kind, text, created_at) VALUES ('b2','sess1','agent','first response','2020-01-01T00:00:01Z')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO session_blocks (id, session_id, kind, text, created_at) VALUES ('b3','sess1','agent','final response','2020-01-01T00:00:02Z')",
                [],
            )
            .unwrap();
        }
        let output = fetch_last_output(&db, "sess1").unwrap();
        assert_eq!(output, "final response");
    }
}
