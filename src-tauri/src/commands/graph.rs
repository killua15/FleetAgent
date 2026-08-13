//! Commands for the code knowledge graph (`GRAPHIFY_TRD.md`, Phase 1).
//!
//! Everything here degrades to nothing when `graphify` isn't installed: the
//! status command reports it as missing and the UI says so. That is the whole
//! contract of §3's "not mandatory" — same treatment Codex and Gemini get.
//!
//! The rules that protect the user's machine (`--code-only`, `--out`, one
//! build per scope) live in `crate::graph`, next to the tests that hold them.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::db::DbConnection;
use crate::graph::{self, ScopeKind};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

/// Building a medium repo took 6 s in the Phase 0 spike, but that was warm and
/// on this machine. A large monorepo on a cold cache is a different animal, and
/// the shared 25 s default would kill it halfway.
const BUILD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// Whether the tool is on the machine, and what to run if not.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphifyStatus {
    pub installed: bool,
    /// `graphify-mcp` ships in the same package, but a partial install is
    /// confusing enough to report on its own: the graph would build and then
    /// never reach an agent.
    pub mcp_installed: bool,
    pub binary_path: Option<String>,
    pub install_hint: String,
}

/// One scope's graph, as the UI needs it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphState {
    pub scope_kind: String,
    pub scope_id: String,
    /// `never_built` | `building` | `ready` | `failed`
    pub status: String,
    pub built_at: Option<String>,
    pub node_count: i64,
    pub edge_count: i64,
    pub last_error: Option<String>,
    /// The commit the graph was built over, per member project.
    pub commits: Vec<GraphCommit>,
    /// True when any member has moved since the build. §5.5: a stale graph
    /// answers with the shape of the truth and the wrong content, so the age
    /// travels with every answer rather than being inferred by the caller.
    pub stale: bool,
    /// Present only once a build has produced it — graphify's own HTML, which
    /// Hangar opens in the system browser rather than embedding (§5.8).
    pub html_path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCommit {
    pub project_id: String,
    pub project_name: String,
    /// What the graph was built over.
    pub built_sha: String,
    /// What the checkout is on now. `None` when the project has no git repo,
    /// or the directory is gone.
    pub current_sha: Option<String>,
    pub stale: bool,
}

#[tauri::command]
pub fn graphify_status() -> GraphifyStatus {
    let binary = graph::detect();
    GraphifyStatus {
        installed: binary.is_some(),
        mcp_installed: graph::detect_mcp().is_some(),
        binary_path: binary,
        install_hint: graph::INSTALL_HINT.to_string(),
    }
}

/// Current `HEAD` of a checkout, or `None` when it isn't a git repo.
///
/// Never an error: a project can legitimately have no repo (Hangar supports
/// `worktree_kind='copy'`), and that must read as "freshness unknown" rather
/// than taking the whole status call down.
fn head_sha(dir: &Path) -> Option<String> {
    crate::commands::pull_requests::git_capture(dir, &["rev-parse", "HEAD"]).ok().filter(|sha| !sha.is_empty())
}

fn read_state(conn: &Connection, app_data: &Path, kind: ScopeKind, scope_id: &str) -> Result<GraphState, String> {
    let row = conn
        .query_row(
            "SELECT status, built_at, node_count, edge_count, last_error
               FROM graphs WHERE scope_kind = ?1 AND scope_id = ?2",
            params![kind.as_str(), scope_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|e| e.to_string())?;

    let (status, built_at, node_count, edge_count, last_error) =
        row.unwrap_or_else(|| ("never_built".to_string(), None, 0, 0, None));

    let mut stmt = conn
        .prepare(
            "SELECT gc.project_id, p.name, gc.commit_sha, p.path
               FROM graph_commits gc JOIN projects p ON p.id = gc.project_id
              WHERE gc.scope_kind = ?1 AND gc.scope_id = ?2
              ORDER BY p.name",
        )
        .map_err(|e| e.to_string())?;
    let commits: Vec<GraphCommit> = stmt
        .query_map(params![kind.as_str(), scope_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .map(|(project_id, project_name, built_sha, path)| {
            let current_sha = head_sha(Path::new(&path));
            // Unknown current sha is not stale: a project without git has no
            // way to be judged, and calling it old would nag forever.
            let stale = current_sha.as_ref().is_some_and(|now| now != &built_sha);
            GraphCommit { project_id, project_name, built_sha, current_sha, stale }
        })
        .collect();

    // §5.5: the group's freshness is its worst member's. Half-fresh does not
    // exist, because the question a group answers spans all of them.
    let stale = commits.iter().any(|c| c.stale);
    let html = graph::graph_html(&graph::scope_dir(app_data, kind, scope_id));

    Ok(GraphState {
        scope_kind: kind.as_str().to_string(),
        scope_id: scope_id.to_string(),
        status,
        built_at,
        node_count,
        edge_count,
        last_error,
        commits,
        stale,
        html_path: html.is_file().then(|| html.to_string_lossy().into_owned()),
    })
}

#[tauri::command]
pub fn get_project_graph(app: AppHandle, db: State<DbConnection>, project_id: String) -> Result<GraphState, String> {
    let app_data = crate::app_data::root(&app)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    read_state(&conn, &app_data, ScopeKind::Project, &project_id)
}

/// Builds (or rebuilds) a project's graph.
///
/// One call the user waits on with a spinner rather than a background job with
/// progress: the spike measured 6 s for this repo, and machinery for a wait
/// nobody has yet is how you get a progress bar that lies. What it can't be is
/// a *synchronous* command — 6 s of running `graphify` on the thread that draws
/// the window is 6 s of a frozen app, so the wait happens on the blocking pool.
#[tauri::command]
pub async fn build_project_graph(app: AppHandle, project_id: String) -> Result<GraphState, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        build_project_graph_inner(&app, &db, &project_id)
    })
    .await?
}

/// `build_project_graph`'s body, reachable without a command invocation so the
/// post-merge rebuild can run it on a background thread.
fn build_project_graph_inner<R: tauri::Runtime>(app: &AppHandle<R>, db: &DbConnection, project_id: &str) -> Result<GraphState, String> {
    let binary = graph::detect()
        .ok_or_else(|| format!("graphify isn't installed — run `{}` and reopen this screen", graph::INSTALL_HINT))?;

    let app_data = crate::app_data::root(&app)?;
    let (repo_path, _name) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT path, name FROM projects WHERE id = ?1", params![project_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(|_| format!("no project with id {project_id}"))?
    };
    let repo = PathBuf::from(&repo_path);
    if !repo.is_dir() {
        return Err(format!("{repo_path} is not a directory anymore"));
    }

    // §5.1. Dropped at the end of the function (or on any `?`), so a failure
    // can't leave the scope permanently unbuildable.
    let _slot = graph::claim_build(ScopeKind::Project, &project_id)
        .ok_or_else(|| "this graph is already building".to_string())?;

    let out = graph::scope_dir(&app_data, ScopeKind::Project, &project_id);
    std::fs::create_dir_all(&out).map_err(|e| format!("could not create {}: {e}", out.display()))?;

    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        mark_building(&conn, ScopeKind::Project, &project_id, &graph::graph_json(&out))?;
    }

    let mut cmd = Command::new(&binary);
    cmd.args(graph::build_args(&repo, &out));
    let output = crate::process_timeout::run_with_timeout(cmd, BUILD_TIMEOUT);

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().to_rfc3339();

    match output {
        Ok(out_ok) if out_ok.status.success() => {
            let stdout = String::from_utf8_lossy(&out_ok.stdout);
            let counts = graph::parse_counts(&stdout);
            conn.execute(
                "UPDATE graphs SET status = 'ready', built_at = ?3, node_count = ?4, edge_count = ?5, last_error = NULL
                   WHERE scope_kind = ?1 AND scope_id = ?2",
                params![ScopeKind::Project.as_str(), project_id, now, counts.nodes, counts.edges],
            )
            .map_err(|e| e.to_string())?;

            // Record what this was built over, so §5.5 can judge its age later
            // without guessing. A project with no git repo records nothing,
            // and reads as "freshness unknown" rather than permanently stale.
            if let Some(sha) = head_sha(&repo) {
                conn.execute(
                    "INSERT INTO graph_commits (scope_kind, scope_id, project_id, commit_sha) VALUES (?1, ?2, ?3, ?4)
                       ON CONFLICT (scope_kind, scope_id, project_id) DO UPDATE SET commit_sha = excluded.commit_sha",
                    params![ScopeKind::Project.as_str(), project_id, project_id, sha],
                )
                .map_err(|e| e.to_string())?;
            }

            // A lone project has no seam, so the brief is hubs only (Phase 5).
            write_brief(&conn, &out, &graph::graph_json(&out), &[], &[project_id.to_string()]);
        }
        Ok(failed) => {
            let stderr = String::from_utf8_lossy(&failed.stderr);
            let detail = stderr.lines().last().unwrap_or("graphify exited with an error").trim().to_string();
            record_failure(&conn, &project_id, &detail)?;
            return Err(format!("the graph build failed: {detail}"));
        }
        Err(e) => {
            record_failure(&conn, &project_id, &e)?;
            return Err(format!("the graph build failed: {e}"));
        }
    }

    read_state(&conn, &app_data, ScopeKind::Project, &project_id)
}

/// Rebuilds a project's graph in the background after something merged to its
/// base branch (`GRAPHIFY_TRD.md` §5.1).
///
/// A merge is when the base moved under every worktree, so it is the one event
/// where the graph reliably stops describing the repo. Explicitly **not** per
/// turn: a rebuild racing six working agents is CPU stolen from them.
///
/// On its own thread, because the caller is the PR poller and a six-second
/// index has no business blocking it. Silent when there is no graph to refresh,
/// and `claim_build` turns it away when one is already running — so several PRs
/// landing together produce one rebuild, not one each.
pub fn rebuild_after_merge<R: tauri::Runtime>(app: &AppHandle<R>, project_id: &str) {
    let has_graph = app
        .try_state::<DbConnection>()
        .and_then(|db| db.0.lock().ok().map(|conn| effective_scope(&conn, project_id).is_some()))
        .unwrap_or(false);
    if !has_graph {
        return;
    }

    let app = app.clone();
    let project_id = project_id.to_string();
    std::thread::spawn(move || {
        let Some(db) = app.try_state::<DbConnection>() else { return };
        match build_project_graph_inner(&app, &db, &project_id) {
            Ok(_) => log::info!("rebuild_after_merge: refreshed the graph for {project_id}"),
            // A failed background rebuild leaves the previous graph in place and
            // marked stale, which is the honest state — and the user still has
            // the button. Nothing here is worth interrupting them over.
            Err(e) => log::warn!("rebuild_after_merge: could not refresh {project_id}: {e}"),
        }
    });
}

/// Renders the architecture brief for a freshly built scope and puts it where
/// both an agent and the user can find it (Phase 5).
///
/// Two destinations, on purpose. The file next to the graph is what actually
/// reaches a turn (`preferences::instruction_file` reads it at spawn). The
/// `memory_files` row is what makes it *visible* — the user can read what their
/// agents are being told, which is the difference between context and a black
/// box.
///
/// Best-effort throughout: a build that produced a working graph must not fail
/// because a summary of it could not be written.
fn write_brief(conn: &Connection, scope_dir: &Path, graph_path: &Path, bridges: &[graph::Bridge], project_ids: &[String]) {
    let brief = match graph::architecture_brief(graph_path, bridges, 12) {
        Ok(brief) => brief,
        Err(e) => {
            log::warn!("write_brief: could not render the architecture brief: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(graph::brief_path(scope_dir), &brief) {
        log::warn!("write_brief: could not write the brief: {e}");
        return;
    }
    for project_id in project_ids {
        let result = conn.execute(
            "INSERT INTO memory_files (id, project_id, filename, content_md, updated_at)
             VALUES (?1, ?2, 'GRAPH_BRIEF.md', ?3, ?4)
             ON CONFLICT (id) DO NOTHING",
            params![
                // Derived from the project rather than random, so a rebuild
                // replaces the row instead of stacking a new copy every time.
                format!("graph-brief-{project_id}"),
                project_id,
                brief,
                Utc::now().to_rfc3339()
            ],
        );
        // The insert above no-ops on a rebuild; the update is what refreshes it.
        if result.is_ok() {
            let _ = conn.execute(
                "UPDATE memory_files SET content_md = ?2, updated_at = ?3 WHERE id = ?1",
                params![format!("graph-brief-{project_id}"), brief, Utc::now().to_rfc3339()],
            );
        }
    }
}

fn mark_building(conn: &Connection, kind: ScopeKind, scope_id: &str, graph_path: &Path) -> Result<(), String> {
    conn.execute(
        "INSERT INTO graphs (scope_kind, scope_id, graph_path, status) VALUES (?1, ?2, ?3, 'building')
           ON CONFLICT (scope_kind, scope_id) DO UPDATE SET status = 'building', graph_path = excluded.graph_path",
        params![kind.as_str(), scope_id, graph_path.to_string_lossy()],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

fn record_failure(conn: &Connection, scope_id: &str, detail: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE graphs SET status = 'failed', last_error = ?3 WHERE scope_kind = ?1 AND scope_id = ?2",
        params![ScopeKind::Project.as_str(), scope_id, detail],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

/// Opens graphify's own interactive HTML in the system browser.
///
/// Not embedded in a panel, and not because of layout: it loads `vis-network`
/// from `unpkg.com`, so under the app's CSP it renders blank, and offline it
/// renders blank anywhere (§5.8). In the system browser those are the browser's
/// problem, and the user gets the full view they were promised.
#[tauri::command]
pub fn open_graph_html(app: AppHandle, project_id: String) -> Result<(), String> {
    let app_data = crate::app_data::root(&app)?;
    let html = graph::graph_html(&graph::scope_dir(&app_data, ScopeKind::Project, &project_id));
    if !html.is_file() {
        return Err("this graph has no visualization yet — build it first".to_string());
    }
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_path(html.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("could not open the graph: {e}"))
}

/// Writes the `--mcp-config` file for a scope and returns its path, or `None`
/// when there is nothing worth serving.
///
/// Returning `None` is the important half. A graph that never built, or whose
/// MCP server isn't installed, must not be registered: §5.5's rule is that an
/// agent asking a graph it shouldn't trust is worse off than one that never
/// asked. Callers pass no flag at all in that case.
pub fn mcp_config_path<R: tauri::Runtime>(app: &AppHandle<R>, kind: ScopeKind, scope_id: &str) -> Option<PathBuf> {
    let mcp_binary = graph::detect_mcp()?;
    let app_data = crate::app_data::root(app).ok()?;
    let scope = graph::scope_dir(&app_data, kind, scope_id);
    let graph_json = graph::graph_json(&scope);
    if !graph_json.is_file() {
        return None;
    }

    let config = scope.join("mcp-config.json");
    std::fs::write(&config, graph::mcp_config_json(&mcp_binary, &graph_json)).ok()?;
    Some(config)
}

/// Where the app data lives, for the UI to show alongside "we never write in
/// your repo" — a claim is easier to believe when you can see the path.
#[tauri::command]
pub fn graph_storage_dir(app: AppHandle) -> Result<String, String> {
    Ok(crate::app_data::root(&app)?.join("graphs").to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Phase 2 — groups (TRD §5.6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphGroup {
    pub id: String,
    pub name: String,
    pub project_count: i64,
    /// How many shared type names cross the seam. **Shown even when zero**:
    /// a group that bought nothing has to be visible in the list, not
    /// discovered by opening the graph (§5.7). `None` means never built,
    /// which is a different thing from a real zero.
    pub bridge_count: Option<i64>,
}

#[tauri::command]
pub fn list_graph_groups(db: State<DbConnection>) -> Result<Vec<GraphGroup>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT g.id, g.name, g.bridge_count,
                    (SELECT COUNT(*) FROM graph_group_projects gp WHERE gp.group_id = g.id)
               FROM graph_groups g ORDER BY g.created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let bridges: i64 = r.get(2)?;
            Ok(GraphGroup {
                id: r.get(0)?,
                name: r.get(1)?,
                bridge_count: (bridges >= 0).then_some(bridges),
                project_count: r.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

#[tauri::command]
pub fn create_graph_group(db: State<DbConnection>, name: String) -> Result<GraphGroup, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("a group needs a name".to_string());
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO graph_groups (id, name, created_at) VALUES (?1, ?2, ?3)",
        params![id, trimmed, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(GraphGroup { id, name: trimmed.to_string(), project_count: 0, bridge_count: None })
}

#[tauri::command]
pub fn delete_graph_group(app: AppHandle, db: State<DbConnection>, group_id: String) -> Result<(), String> {
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM graph_groups WHERE id = ?1", params![group_id]).map_err(|e| e.to_string())?;
        conn.execute(
            "DELETE FROM graphs WHERE scope_kind = 'group' AND scope_id = ?1",
            params![group_id],
        )
        .map_err(|e| e.to_string())?;
    }
    // Deleting a group has to take its built graph with it, or the app data
    // grows a megabyte per group the user ever tried and then abandoned.
    if let Ok(root) = crate::app_data::root(&app) {
        let _ = std::fs::remove_dir_all(graph::scope_dir(&root, ScopeKind::Group, &group_id));
    }
    Ok(())
}

/// Adds or removes a project from a group.
///
/// The group's graph is **not** rebuilt here: §8.5. Rebuilding on a settings
/// click spends CPU the moment someone is browsing, and the honest state after
/// a membership change is "stale", not "wrong". What *does* happen immediately
/// is that the bridge count is invalidated — a stale number next to a changed
/// membership is worse than no number.
#[tauri::command]
pub fn set_graph_group_member(db: State<DbConnection>, group_id: String, project_id: String, member: bool) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if member {
        conn.execute(
            "INSERT OR IGNORE INTO graph_group_projects (group_id, project_id) VALUES (?1, ?2)",
            params![group_id, project_id],
        )
        .map_err(|e| e.to_string())?;
    } else {
        conn.execute(
            "DELETE FROM graph_group_projects WHERE group_id = ?1 AND project_id = ?2",
            params![group_id, project_id],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute("UPDATE graph_groups SET bridge_count = -1 WHERE id = ?1", params![group_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_graph_group_members(db: State<DbConnection>, group_id: String) -> Result<Vec<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT project_id FROM graph_group_projects WHERE group_id = ?1")
        .map_err(|e| e.to_string())?;
    let ids = stmt
        .query_map(params![group_id], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(ids)
}

#[tauri::command]
pub fn get_graph_group_state(app: AppHandle, db: State<DbConnection>, group_id: String) -> Result<GraphState, String> {
    let app_data = crate::app_data::root(&app)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    read_state(&conn, &app_data, ScopeKind::Group, &group_id)
}

/// The bridges of a built group graph — the seam map's data (§5.8).
#[tauri::command]
pub fn get_graph_bridges(app: AppHandle, group_id: String) -> Result<Vec<graph::Bridge>, String> {
    let app_data = crate::app_data::root(&app)?;
    let merged = graph::graph_json(&graph::scope_dir(&app_data, ScopeKind::Group, &group_id));
    if !merged.is_file() {
        return Ok(Vec::new());
    }
    Ok(graph::find_bridges(&graph::read_merged_nodes(&merged)?))
}

/// Builds a group's graph: one `extract` per member, then graphify's own
/// `merge-graphs`, then our bridges (§5.6, §5.7).
/// Off the UI thread for the same reason as `build_project_graph`, and more so:
/// this runs one `extract` per member project before merging them.
#[tauri::command]
pub async fn build_graph_group(app: AppHandle, group_id: String) -> Result<GraphState, String> {
    crate::blocking::run_blocking(move || build_graph_group_blocking(app, group_id)).await?
}

fn build_graph_group_blocking(app: AppHandle, group_id: String) -> Result<GraphState, String> {
    let db = app.state::<DbConnection>();
    let binary = graph::detect()
        .ok_or_else(|| format!("graphify isn't installed — run `{}` and reopen this screen", graph::INSTALL_HINT))?;
    let app_data = crate::app_data::root(&app)?;

    let members: Vec<(String, String)> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT p.id, p.path FROM graph_group_projects gp JOIN projects p ON p.id = gp.project_id
                  WHERE gp.group_id = ?1 ORDER BY p.name",
            )
            .map_err(|e| e.to_string())?;
        let rows: Vec<(String, String)> = stmt
            .query_map(params![group_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        rows
    };
    if members.len() < 2 {
        return Err("a graph group needs at least two projects — that is the whole point of it".to_string());
    }

    let _slot = graph::claim_build(ScopeKind::Group, &group_id).ok_or_else(|| "this graph is already building".to_string())?;

    let group_dir = graph::scope_dir(&app_data, ScopeKind::Group, &group_id);
    std::fs::create_dir_all(&group_dir).map_err(|e| format!("could not create {}: {e}", group_dir.display()))?;
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        mark_building(&conn, ScopeKind::Group, &group_id, &graph::graph_json(&group_dir))?;
    }

    // Each member is indexed into its own directory, exactly as a lone project
    // would be. Members are extracted in series, not in parallel: graphify
    // already uses every core (10 workers on this machine), so running two at
    // once would only make them fight each other.
    let mut member_graphs = Vec::new();
    let mut shas = Vec::new();
    for (project_id, repo_path) in &members {
        let repo = PathBuf::from(repo_path);
        if !repo.is_dir() {
            return fail_group(&db, &group_id, &format!("{repo_path} is not a directory anymore"));
        }
        let member_out = graph::scope_dir(&app_data, ScopeKind::Project, project_id);
        std::fs::create_dir_all(&member_out).map_err(|e| e.to_string())?;

        let mut cmd = Command::new(&binary);
        cmd.args(graph::build_args(&repo, &member_out));
        match crate::process_timeout::run_with_timeout(cmd, BUILD_TIMEOUT) {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let detail = String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("graphify failed").trim().to_string();
                return fail_group(&db, &group_id, &detail);
            }
            Err(e) => return fail_group(&db, &group_id, &e),
        }
        member_graphs.push(graph::graph_json(&member_out));
        shas.push((project_id.clone(), head_sha(&repo)));
    }

    let merged = graph::graph_json(&group_dir);
    if let Some(parent) = merged.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut merge = Command::new(&binary);
    merge.args(graph::merge_args(&member_graphs, &merged));
    match crate::process_timeout::run_with_timeout(merge, BUILD_TIMEOUT) {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let detail = String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("merge-graphs failed").trim().to_string();
            return fail_group(&db, &group_id, &detail);
        }
        Err(e) => return fail_group(&db, &group_id, &e),
    }

    let nodes = graph::read_merged_nodes(&merged)?;
    let bridges = graph::find_bridges(&nodes);

    // The bridges go into the graph itself, not just the screen: `merge-graphs`
    // leaves zero edges crossing between repos, so without this the MCP would
    // serve a graph in which the seam does not exist (§5.7).
    let bridge_edges = graph::inject_bridges(&merged, &bridges)?;

    // Re-cluster so communities can finally span both repos — they could not
    // before, because Leiden works on edges and there were none across. This is
    // also what writes `graph.html`, which `merge-graphs` never does.
    if bridge_edges > 0 {
        let mut recluster = Command::new(&binary);
        recluster.args(graph::recluster_args(&group_dir));
        match crate::process_timeout::run_with_timeout(recluster, BUILD_TIMEOUT) {
            Ok(o) if o.status.success() => {}
            // A failed re-clustering is not a failed build: the graph is whole
            // and queryable, it just has no fresh communities or HTML. Losing
            // the whole index over the cosmetic half would be the wrong trade.
            Ok(o) => log::warn!("build_graph_group: cluster-only failed: {}", String::from_utf8_lossy(&o.stderr)),
            Err(e) => log::warn!("build_graph_group: cluster-only failed: {e}"),
        }
    }

    let edge_count = graph::count_edges(&merged).unwrap_or(0);

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE graphs SET status = 'ready', built_at = ?2, node_count = ?3, edge_count = ?4, last_error = NULL
           WHERE scope_kind = 'group' AND scope_id = ?1",
        params![group_id, Utc::now().to_rfc3339(), nodes.len() as i64, edge_count],
    )
    .map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE graph_groups SET bridge_count = ?2 WHERE id = ?1",
        params![group_id, bridges.len() as i64],
    )
    .map_err(|e| e.to_string())?;

    // Freshness is per member and the worst one wins (§5.5), so every member's
    // commit is recorded, not just the last.
    conn.execute("DELETE FROM graph_commits WHERE scope_kind = 'group' AND scope_id = ?1", params![group_id])
        .map_err(|e| e.to_string())?;
    for (project_id, sha) in shas.into_iter() {
        if let Some(sha) = sha {
            conn.execute(
                "INSERT INTO graph_commits (scope_kind, scope_id, project_id, commit_sha) VALUES ('group', ?1, ?2, ?3)",
                params![group_id, project_id, sha],
            )
            .map_err(|e| e.to_string())?;
        }
    }

    // Every member gets the same brief: the seam is what they share, and an
    // agent on either side needs to know both ends of it.
    let member_ids: Vec<String> = members.iter().map(|(id, _)| id.clone()).collect();
    write_brief(&conn, &group_dir, &merged, &bridges, &member_ids);

    read_state(&conn, &app_data, ScopeKind::Group, &group_id)
}

fn fail_group(db: &State<DbConnection>, group_id: &str, detail: &str) -> Result<GraphState, String> {
    if let Ok(conn) = db.0.lock() {
        let _ = conn.execute(
            "UPDATE graphs SET status = 'failed', last_error = ?2 WHERE scope_kind = 'group' AND scope_id = ?1",
            params![group_id, detail],
        );
    }
    Err(format!("the group graph build failed: {detail}"))
}

/// Which graph a project's agents should actually be given.
///
/// **This is what makes grouping mean anything.** Pinning the spawn sites to
/// `ScopeKind::Project` would hand a grouped project its own solitary graph —
/// so the seam would be drawn in the settings screen and be invisible to every
/// agent, which is the one thing grouping exists to fix.
///
/// A project in several groups gets the one with the most bridges: it is a rare
/// case, and "the graph that connects the most" is the least surprising answer.
/// Falls back to the project's own graph, then to nothing.
pub fn effective_scope(conn: &Connection, project_id: &str) -> Option<(ScopeKind, String)> {
    let group: Option<String> = conn
        .query_row(
            "SELECT g.id FROM graph_groups g
               JOIN graph_group_projects gp ON gp.group_id = g.id
               JOIN graphs gr ON gr.scope_kind = 'group' AND gr.scope_id = g.id
              WHERE gp.project_id = ?1 AND gr.status = 'ready'
              ORDER BY g.bridge_count DESC LIMIT 1",
            params![project_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    if let Some(group_id) = group {
        return Some((ScopeKind::Group, group_id));
    }

    let own: Option<i64> = conn
        .query_row(
            "SELECT 1 FROM graphs WHERE scope_kind = 'project' AND scope_id = ?1 AND status = 'ready'",
            params![project_id],
            |r| r.get(0),
        )
        .optional()
        .ok()
        .flatten();
    own.map(|_| (ScopeKind::Project, project_id.to_string()))
}

/// [`mcp_config_path`] for whichever scope this project should actually use.
pub fn mcp_config_for_project<R: tauri::Runtime>(app: &AppHandle<R>, conn: &Connection, project_id: &str) -> Option<PathBuf> {
    let (kind, scope_id) = effective_scope(conn, project_id)?;
    mcp_config_path(app, kind, &scope_id)
}

/// Sibling projects a given project shares a group with.
///
/// Feeds `--add-dir` at spawn time (§5.6): a graph that tells an agent about
/// code it cannot open is a cruelty. Uses the flag Hangar already passes for
/// skills, which is documented there as granting file access too.
pub fn sibling_dirs(conn: &Connection, project_id: &str) -> Vec<PathBuf> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT DISTINCT p.path FROM graph_group_projects mine
           JOIN graph_group_projects theirs ON theirs.group_id = mine.group_id
           JOIN projects p ON p.id = theirs.project_id
          WHERE mine.project_id = ?1 AND theirs.project_id != ?1",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![project_id], |r| r.get::<_, String>(0))
        .map(|rows| rows.filter_map(Result::ok).map(PathBuf::from).filter(|p| p.is_dir()).collect())
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Phase 4 — semantic collision (TRD §6, Differential A)
// ---------------------------------------------------------------------------

/// Another live session working the same shared type.
///
/// This is the capability nobody can copy by installing the same skill: it
/// needs the graph *and* the fleet, and only Hangar knows the fleet.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphCollision {
    pub session_id: String,
    pub session_name: String,
    pub branch: Option<String>,
    pub project_name: String,
    /// The shared type both sides are touching, e.g. `PermissionRequest`.
    pub shared_type: String,
    /// The file on each side, so the warning names something openable rather
    /// than an abstraction.
    pub files: Vec<String>,
}

/// Warns, at subsession creation, that another agent is on the other side of a
/// contract this task touches.
///
/// **Warn, never block** (§6). Two agents on both sides of a type is sometimes
/// exactly what the user wants; Hangar says what it knows and the user decides.
/// A false positive is tolerable, silence about a real collision is not.
///
/// Matching is by shared-type name against the task's text. Crude on purpose:
/// the alternative is asking a model what the task is about, which costs quota
/// on every dialog open and is the sort of thing §3 keeps out.
#[tauri::command]
pub fn graph_collisions(app: AppHandle, db: State<DbConnection>, project_id: String, task_text: String) -> Result<Vec<GraphCollision>, String> {
    let needle = task_text.to_lowercase();
    if needle.trim().is_empty() {
        return Ok(Vec::new());
    }

    let app_data = crate::app_data::root(&app)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let Some((kind, scope_id)) = effective_scope(&conn, &project_id) else {
        return Ok(Vec::new());
    };
    let merged = graph::graph_json(&graph::scope_dir(&app_data, kind, &scope_id));
    if !merged.is_file() {
        return Ok(Vec::new());
    }
    let bridges = graph::find_bridges(&graph::read_merged_nodes(&merged)?);

    // Which shared types this task appears to be about.
    let touched: Vec<&graph::Bridge> = bridges.iter().filter(|b| needle.contains(&b.label.to_lowercase())).collect();
    if touched.is_empty() {
        return Ok(Vec::new());
    }

    // Live sessions elsewhere in the same scope. `running` only: a finished
    // session is not a collision, it is history.
    let mut stmt = conn
        .prepare(
            "SELECT s.id, s.name, s.branch, p.name FROM sessions s
               JOIN projects p ON p.id = s.project_id
              WHERE s.status = 'running'
                AND (s.project_id = ?1
                  OR s.project_id IN (SELECT project_id FROM graph_group_projects WHERE group_id = ?2))",
        )
        .map_err(|e| e.to_string())?;
    let live: Vec<(String, String, Option<String>, String)> = stmt
        .query_map(params![project_id, scope_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();

    Ok(live
        .into_iter()
        .filter_map(|(session_id, session_name, branch, project_name)| {
            // The other session counts as colliding when its own name mentions
            // the same shared type — the same crude, free test applied to both
            // ends, so the rule is symmetric and explainable.
            let other = session_name.to_lowercase();
            let hit = touched.iter().find(|b| other.contains(&b.label.to_lowercase()))?;
            Some(GraphCollision {
                session_id,
                session_name,
                branch,
                project_name,
                shared_type: hit.label.clone(),
                files: hit.ends.iter().map(|e| e.source_file.clone()).collect(),
            })
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Phase 3 — measurement (TRD §6, Differential B)
// ---------------------------------------------------------------------------

/// What turns actually cost, split by whether a graph was available.
///
/// The honest shape matters more than the numbers: sample sizes are reported
/// alongside the averages so a "saving" drawn from four turns can be seen for
/// what it is. A report that can only deliver good news is not a report.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphSavingsReport {
    pub with_graph: TurnSample,
    pub without_graph: TurnSample,
    /// Average input-token delta, negative when the graph costs *more*. `None`
    /// when either side has no turns to compare.
    pub input_delta: Option<f64>,
    pub cost_delta_usd: Option<f64>,
    /// True when either sample is too small for the delta to mean anything.
    /// The UI says so rather than drawing a chart over four data points.
    pub too_early: bool,
    /// Turns on this project that predate `project_id` being recorded on
    /// headless rows, and so can't be attributed to either side.
    ///
    /// Reported rather than silently dropped: a comparison that quietly ignores
    /// part of the evidence is the kind of report §6 says not to ship.
    pub unattributed_turns: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnSample {
    pub turns: i64,
    pub avg_input_tokens: f64,
    pub avg_output_tokens: f64,
    pub total_cost_usd: f64,
    pub avg_cost_usd: f64,
}

/// Below this many turns on either side, a difference is noise.
const MIN_SAMPLE: i64 = 20;

/// Whether a project's agents currently have a graph to consult — a built one
/// on the project itself, or on any group it belongs to.
///
/// Recorded on every `turn_usage` row as it is written, not derived later:
/// "did this turn have a graph" is a fact about the moment the turn ran, and a
/// graph built or deleted next week must not rewrite last week's history.
pub fn graph_available_for_project(conn: &Connection, project_id: &str) -> bool {
    conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM graphs
            WHERE status = 'ready'
              AND ((scope_kind = 'project' AND scope_id = ?1)
                OR (scope_kind = 'group'
                    AND scope_id IN (SELECT group_id FROM graph_group_projects WHERE project_id = ?1))))",
        params![project_id],
        |r| r.get::<_, i64>(0),
    )
    .map(|found| found == 1)
    .unwrap_or(false)
}

/// Same question, from a session — the shape the headless turn runner has.
pub fn graph_available_for_session(conn: &Connection, session_id: &str) -> bool {
    conn.query_row("SELECT project_id FROM sessions WHERE id = ?1", params![session_id], |r| r.get::<_, String>(0))
        .ok()
        .is_some_and(|project_id| graph_available_for_project(conn, &project_id))
}

#[tauri::command]
pub fn graph_savings_report(db: State<DbConnection>, project_id: String) -> Result<GraphSavingsReport, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let sample = |with_graph: bool| -> Result<TurnSample, String> {
        conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(AVG(input_tokens + COALESCE(cache_read_tokens, 0)), 0),
                    COALESCE(AVG(output_tokens), 0),
                    COALESCE(SUM(cost_usd), 0),
                    COALESCE(AVG(cost_usd), 0)
               FROM turn_usage WHERE project_id = ?1 AND with_graph = ?2",
            params![project_id, if with_graph { 1 } else { 0 }],
            |r| {
                Ok(TurnSample {
                    turns: r.get(0)?,
                    avg_input_tokens: r.get(1)?,
                    avg_output_tokens: r.get(2)?,
                    total_cost_usd: r.get(3)?,
                    avg_cost_usd: r.get(4)?,
                })
            },
        )
        .map_err(|e| e.to_string())
    };

    let with_graph = sample(true)?;
    let without_graph = sample(false)?;
    let comparable = with_graph.turns > 0 && without_graph.turns > 0;

    // Rows written before headless turns recorded a project. They belong to
    // this project's sessions but carry no `project_id`, so the samples above
    // can't see them.
    let unattributed_turns = conn
        .query_row(
            "SELECT COUNT(*) FROM turn_usage tu
               JOIN sessions s ON s.id = tu.session_id
              WHERE tu.project_id IS NULL AND s.project_id = ?1",
            params![project_id],
            |r| r.get(0),
        )
        .unwrap_or(0);

    Ok(GraphSavingsReport {
        unattributed_turns,
        input_delta: comparable.then(|| without_graph.avg_input_tokens - with_graph.avg_input_tokens),
        cost_delta_usd: comparable.then(|| without_graph.avg_cost_usd - with_graph.avg_cost_usd),
        too_early: with_graph.turns < MIN_SAMPLE || without_graph.turns < MIN_SAMPLE,
        with_graph,
        without_graph,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_project(path: &str) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS graphs (
               scope_kind TEXT NOT NULL, scope_id TEXT NOT NULL, graph_path TEXT NOT NULL,
               built_at TEXT, node_count INTEGER NOT NULL DEFAULT 0, edge_count INTEGER NOT NULL DEFAULT 0,
               status TEXT NOT NULL DEFAULT 'never_built', last_error TEXT,
               PRIMARY KEY (scope_kind, scope_id));
             CREATE TABLE IF NOT EXISTS graph_commits (
               scope_kind TEXT NOT NULL, scope_id TEXT NOT NULL, project_id TEXT NOT NULL,
               commit_sha TEXT NOT NULL, PRIMARY KEY (scope_kind, scope_id, project_id));",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('p1', 'Thing', ?1, 'now')",
            params![path],
        )
        .unwrap();
        conn
    }

    /// A project that was never indexed has to read as "never built", not as
    /// an error — the whole screen has to render on a machine without graphify.
    #[test]
    fn a_project_with_no_graph_reports_never_built() {
        let conn = db_with_project("/nowhere");

        let state = read_state(&conn, Path::new("/appdata"), ScopeKind::Project, "p1").unwrap();

        assert_eq!(state.status, "never_built");
        assert!(!state.stale, "nothing built cannot be stale");
        assert!(state.commits.is_empty());
        assert!(state.html_path.is_none());
    }

    /// §5.5: a member that moved makes the whole scope stale. Recorded against
    /// a path with no git repo, so `current_sha` is None — which must read as
    /// *unknown*, not as *old*, or a non-git project nags forever.
    #[test]
    fn a_project_without_git_is_never_reported_stale() {
        let conn = db_with_project("/definitely/not/a/repo");
        conn.execute(
            "INSERT INTO graphs (scope_kind, scope_id, graph_path, status, built_at, node_count, edge_count)
             VALUES ('project', 'p1', '/appdata/graphs/project-p1/graphify-out/graph.json', 'ready', 'yesterday', 2613, 7332)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO graph_commits (scope_kind, scope_id, project_id, commit_sha) VALUES ('project', 'p1', 'p1', 'deadbeef')",
            [],
        )
        .unwrap();

        let state = read_state(&conn, Path::new("/appdata"), ScopeKind::Project, "p1").unwrap();

        assert_eq!(state.status, "ready");
        assert_eq!(state.node_count, 2613);
        assert_eq!(state.commits.len(), 1);
        assert!(state.commits[0].current_sha.is_none(), "a non-repo has no HEAD to compare against");
        assert!(!state.stale, "unknown freshness must not be reported as stale");
    }

    /// The whole chain against two real repositories, once.
    ///
    /// Every other test here proves a piece in isolation — the bridge filter,
    /// the flags, the lock, the freshness rule. None of them has ever run
    /// `graphify`, so nothing so far proves the pieces fit: that the argv we
    /// build is argv graphify accepts, that `merge-graphs` writes where we look
    /// for it, that our injected edges survive its clustering.
    ///
    /// `#[ignore]` like the other tests that need real binaries
    /// (`turn_runner.rs`, `login_manager.rs`, `usage_scraper.rs`). The repos
    /// come from the environment and the test skips without them: one that only
    /// passes on one machine tells nobody anything the day it fails.
    ///
    /// ```sh
    /// HANGAR_GRAPH_TEST_REPO_A=~/Proyects/Fleet-Agent \
    /// HANGAR_GRAPH_TEST_REPO_B=~/Proyects/hangar-mobile \
    ///   cargo test --lib -- --ignored the_whole_chain
    /// ```
    #[test]
    #[ignore = "runs real graphify against two real repos; set HANGAR_GRAPH_TEST_REPO_A/B"]
    fn the_whole_chain_builds_merges_and_bridges_two_real_repos() {
        let (Ok(repo_a), Ok(repo_b)) =
            (std::env::var("HANGAR_GRAPH_TEST_REPO_A"), std::env::var("HANGAR_GRAPH_TEST_REPO_B"))
        else {
            eprintln!("skipped: set HANGAR_GRAPH_TEST_REPO_A and HANGAR_GRAPH_TEST_REPO_B to two repos that talk to each other");
            return;
        };
        let (repo_a, repo_b) = (PathBuf::from(&repo_a), PathBuf::from(&repo_b));
        let binary = graph::detect().expect("graphify has to be installed to run this");

        let root = std::env::temp_dir().join(format!("hangar-graph-chain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // §3's promise, captured before we touch anything. A graph build that
        // dirties the user's checkout is the one failure that can't be caught
        // by reading the code, because it depends on what graphify does.
        let status_of = |repo: &Path| {
            crate::commands::pull_requests::git_capture(repo, &["status", "--porcelain"]).unwrap_or_default()
        };
        let (before_a, before_b) = (status_of(&repo_a), status_of(&repo_b));

        let run = |args: Vec<String>| {
            let mut cmd = Command::new(&binary);
            cmd.args(&args);
            let out = crate::process_timeout::run_with_timeout(cmd, BUILD_TIMEOUT)
                .unwrap_or_else(|e| panic!("graphify {args:?} could not run: {e}"));
            assert!(out.status.success(), "graphify {args:?} failed: {}", String::from_utf8_lossy(&out.stderr));
            out
        };

        // 1 — one graph per member, exactly as `build_project_graph` does it.
        let out_a = root.join("a");
        let out_b = root.join("b");
        std::fs::create_dir_all(&out_a).unwrap();
        std::fs::create_dir_all(&out_b).unwrap();
        run(graph::build_args(&repo_a, &out_a));
        run(graph::build_args(&repo_b, &out_b));

        let graph_a = graph::graph_json(&out_a);
        let graph_b = graph::graph_json(&out_b);
        assert!(graph_a.is_file() && graph_b.is_file(), "both member graphs have to exist where we look for them");

        // 2 — graphify's own merge. Verifies the union we measured in Phase 0:
        // it joins the two graphs and creates nothing between them.
        let merged = root.join("merged").join("graphify-out").join("graph.json");
        std::fs::create_dir_all(merged.parent().unwrap()).unwrap();
        run(graph::merge_args(&[graph_a, graph_b], &merged));

        let nodes = graph::read_merged_nodes(&merged).expect("the merged graph has to parse");
        assert!(nodes.len() > 100, "a merge of two real repos should be substantial, got {}", nodes.len());

        // 3 — the one piece of graph logic we own.
        let bridges = graph::find_bridges(&nodes);
        assert!(
            !bridges.is_empty(),
            "two repos that talk to each other must share at least one type name, or §5.6 buys nothing"
        );
        eprintln!("bridges found: {} — {:?}", bridges.len(), bridges.iter().map(|b| &b.label).collect::<Vec<_>>());

        // 4 — the edges reach the graph the MCP serves, not just the screen.
        let added = graph::inject_bridges(&merged, &bridges).expect("bridges have to be writable");
        assert_eq!(added, bridges.len(), "one edge per two-ended bridge");

        let raw: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&merged).unwrap()).unwrap();
        let ours = raw["links"].as_array().unwrap().iter().filter(|l| l["_origin"] == "hangar-bridge").count();
        assert_eq!(ours, added, "every injected edge has to be findable and marked as ours");

        // 5 — re-clustering, which is also what writes the HTML that
        // `merge-graphs` never produces. `--no-label` keeps it off any model.
        run(graph::recluster_args(&root.join("merged")));
        assert!(
            graph::graph_html(&root.join("merged")).is_file(),
            "cluster-only is the only thing that writes graph.html; without it the 'open in browser' button never appears"
        );

        // 6 — the brief, and the size that justified not shipping the 25 KB
        // report graphify generates (Phase 5).
        let brief = graph::architecture_brief(&merged, &bridges, 12).expect("the brief has to render");
        assert!(!brief.trim().is_empty());
        assert!(
            brief.len() < 4096,
            "the brief rides every turn's system prompt; at {} bytes it stops being cheaper than the greps it replaces",
            brief.len()
        );

        // 7 — and the promise again, after all of it.
        assert_eq!(status_of(&repo_a), before_a, "building a graph must not touch the user's checkout");
        assert_eq!(status_of(&repo_b), before_b, "building a graph must not touch the user's checkout");

        std::fs::remove_dir_all(&root).ok();
    }
}
