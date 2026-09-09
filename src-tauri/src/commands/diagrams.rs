//! Listing and opening the diagrams an agent delivered for a project.

use crate::diagrams::{self, Diagram};
use tauri::AppHandle;

/// What has been delivered for this project, newest first.
///
/// Empty for the common case — most projects will never have asked for a
/// diagram — and empty is not an error worth surfacing.
#[tauri::command]
pub fn list_project_diagrams(app: AppHandle, project_id: String) -> Result<Vec<Diagram>, String> {
    let app_data = crate::app_data::root(&app)?;
    Ok(diagrams::list(&diagrams::dir(&app_data, &project_id)))
}

/// Opens one delivered page in the system browser.
///
/// The browser and not a panel, for the same reason `open_graph_html` gives
/// (`commands/graph.rs`): a page like this renders blank under the app's CSP.
/// Here there is a second reason — the delivered HTML is written by whatever an
/// agent authored, and the system browser is a much better place for that than
/// inside the app's own webview.
///
/// `file_name` is the only value in this feature that comes from the frontend
/// and ends up in a system call, so it is validated rather than trusted.
#[tauri::command]
pub fn open_diagram(app: AppHandle, project_id: String, file_name: String) -> Result<(), String> {
    if !diagrams::is_safe_file_name(&file_name) {
        return Err(format!("'{file_name}' is not a diagram in this project"));
    }
    let app_data = crate::app_data::root(&app)?;
    let path = diagrams::dir(&app_data, &project_id).join(&file_name);
    if !path.is_file() {
        return Err(format!("'{file_name}' is not there any more — the list may be stale"));
    }
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("could not open the diagram: {e}"))
}

/// Asks an agent to draw this project, or to bring an existing drawing up to
/// date.
///
/// Modelled on `trigger_review` (`commands/settings.rs`), which is the other
/// place Hangar starts a turn on the user's behalf: create a session so the
/// work has a transcript to watch and somewhere for a failure to land, then
/// hand `spawn_turn` everything the same project's terminal would have had.
///
/// **The account is a parameter, not a setting.** This spends real quota, and
/// picking it at the moment of asking is more honest than a preference set once
/// and forgotten — the picker is right next to the button.
///
/// `spec_file_name` is what makes "update" different from "draw again". archify
/// authors a typed JSON spec and compiles the page from it, so an update is an
/// edit to that spec — regenerating from nothing would throw away every layout
/// decision the last run made and produce a different picture of the same
/// system.
#[tauri::command]
pub async fn generate_project_diagram<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    project_id: String,
    account_id: String,
    spec_file_name: Option<String>,
) -> Result<String, String> {
    crate::blocking::run_blocking(move || generate_blocking(app, project_id, account_id, spec_file_name)).await?
}

fn generate_blocking<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    project_id: String,
    account_id: String,
    spec_file_name: Option<String>,
) -> Result<String, String> {
    use rusqlite::params;
    use tauri::Manager;

    if let Some(name) = spec_file_name.as_deref() {
        // Same rule as `open_diagram`: this reaches a shell as part of a
        // prompt, so it is checked rather than trusted.
        if !diagrams::is_safe_spec_name(name) {
            return Err(format!("'{name}' is not a spec in this project"));
        }
    }

    let db = app.state::<crate::db::DbConnection>();
    let app_data = crate::app_data::root(&app)?;

    let (project_name, cwd): (String, String) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT name, path FROM projects WHERE id = ?1", params![project_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("project lookup failed: {e}"))?
    };
    let (provider, billing, config_dir_path): (String, String, String) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT provider, billing, config_dir_path FROM accounts WHERE id = ?1", params![account_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| format!("account lookup failed: {e}"))?
    };

    // The skill has to be in play, and it is checked against the symlinks that
    // will actually be handed over rather than the saved ticks — the same
    // source `terminals.rs` reads. Refusing here beats spending a turn on an
    // agent that has never heard of archify.
    let skills_dir = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::commands::skills::skills_dir(&app_data, &conn, &project_id)?
    };
    let linked = skills_dir.as_deref().map(crate::commands::skills::linked_skills).unwrap_or_default();
    if !linked.iter().any(|s| s == crate::commands::skills::ARCHIFY) {
        return Err("turn on the `archify` skill for this project first — Settings → Skills".to_string());
    }

    let api_key = if billing == "key" { Some(crate::credential_vault::get(&account_id)?) } else { None };
    let mut env = crate::providers::claude::env_for_account(&config_dir_path, api_key.as_deref());
    env.extend(crate::commands::skills::silencing_env(&linked));
    env.extend(crate::commands::skills::diagrams_env(&linked, &app_data, &project_id));

    let (instructions, mcp_config) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let instructions = crate::commands::preferences::instruction_file(&app_data, &conn, Some(&project_id))
            .unwrap_or_else(|e| {
                log::warn!("generate_project_diagram: could not prepare the agent preferences — {e}");
                None
            })
            .map(|resolved| resolved.file);
        (instructions, crate::commands::graph::mcp_config_for_project(&app, &conn, &project_id))
    };

    let updating = spec_file_name.is_some();
    let prompt = match spec_file_name.as_deref() {
        Some(spec) => format!(
            "The architecture diagram for this project is out of date. Its archify spec is at \
             `$HANGAR_DIAGRAMS/{spec}`. Read it, compare it against the code as it is now, and edit \
             the spec so it matches — keep the ids, wording and layout of everything that has not \
             changed, so this reads as an update and not a different drawing. Then validate and \
             deliver it to the same file names it already uses.",
        ),
        None => format!(
            "Draw the architecture of {project_name} with the `archify` skill: what the main parts \
             are and how they talk to each other. Follow the skill's own limits on how many nodes a \
             readable diagram has. Deliver it into `$HANGAR_DIAGRAMS`, and leave the spec next to \
             the page so it can be updated later instead of redrawn.",
        ),
    };

    let session_id = uuid::Uuid::new_v4().to_string();
    let name = format!("{} {}", if updating { "Update diagram:" } else { "Diagram:" }, project_name);
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, node_id, parent_session_id, worktree_path, worktree_branch, worktree_kind, account_id, model, effort, mode, status, pid, provider_session_id, started_at, ended_at)
             VALUES (?1, ?2, ?3, NULL, NULL, NULL, NULL, NULL, ?4, '', 'high', 'auto_edit', 'waiting', NULL, NULL, ?5, NULL)",
            params![session_id, name, project_id, account_id, chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| e.to_string())?;
    }

    crate::turn_runner::spawn_turn(crate::turn_runner::TurnRequest {
        app: app.clone(),
        session_id: session_id.clone(),
        account_id,
        provider,
        cwd,
        env,
        // It has to write a file and run `archify.mjs validate`/`deliver`, and
        // it writes only inside Hangar's own diagrams directory. Asking a human
        // to approve each of those defeats the button.
        mode: crate::providers::Mode::AutoEdit,
        model: String::new(),
        effort: "high".to_string(),
        name,
        resume: None,
        prompt,
        instructions,
        mcp_config,
        skills_dir,
    })?;
    Ok(session_id)
}
