use crate::credential_vault;
use crate::db::DbConnection;
use crate::models::{ResumableConversation, Session, SessionBlock, SessionView};
use crate::process_manager;
use crate::providers::{claude, Mode};
use crate::turn_runner::{self, TurnRequest};
use chrono::Utc;
use rusqlite::params;
use serde::Deserialize;
use std::io::BufRead;
use tauri::{AppHandle, State};
use uuid::Uuid;

const SESSION_VIEW_COLUMNS: &str = "s.id, s.name, s.project_id, p.name, s.account_id, a.name, a.provider, a.scope, s.model, s.effort, s.mode, s.status, s.started_at";

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

fn row_to_session(row: &rusqlite::Row) -> rusqlite::Result<Session> {
    Ok(Session {
        id: row.get("id")?,
        name: row.get("name")?,
        project_id: row.get("project_id")?,
        node_id: row.get("node_id")?,
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
        account_id: row.get(4)?,
        account_name: row.get(5)?,
        provider: row.get(6)?,
        scope: row.get(7)?,
        model: row.get(8)?,
        effort: row.get(9)?,
        mode: row.get(10)?,
        status: row.get(11)?,
        started_at: row.get(12)?,
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
    let (provider, billing, config_dir_path, account_id, project_path, name, model, effort, mode_str, resume) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let (account_id, project_id, name, model, effort, mode_str, resume): (
            String,
            String,
            String,
            String,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "SELECT account_id, project_id, name, model, effort, mode, provider_session_id FROM sessions WHERE id = ?1",
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
        (provider, billing, config_dir_path, account_id, project_path, name, model, effort, mode_str, resume)
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
        cwd: project_path,
        env,
        mode: Mode::from_str(&mode_str),
        model,
        effort,
        name,
        resume,
        prompt: text,
    });

    Ok(())
}

#[tauri::command]
pub fn stop_session(app: AppHandle, db: State<DbConnection>, session_id: String) -> Result<(), String> {
    let _ = process_manager::kill(&app, &session_id);
    turn_runner::kill_turn(&app, &session_id);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE sessions SET status = 'done', ended_at = ?2 WHERE id = ?1",
        params![session_id, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Removes a session entirely (not just marks it done) — cascades to its
/// blocks and checkpoints. Kills any tracked live process first, best-effort.
#[tauri::command]
pub fn delete_session(app: AppHandle, db: State<DbConnection>, session_id: String) -> Result<(), String> {
    let _ = process_manager::kill(&app, &session_id);
    turn_runner::kill_turn(&app, &session_id);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])
        .map_err(|e| e.to_string())?;
    Ok(())
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

/// Scans this session's Claude Code config dir for other conversations
/// against the same project, so the user can pick one to continue instead of
/// this session's own conversation — there is no `claude -r`-style
/// interactive picker available to a headless caller, so Fleet reads the
/// same on-disk transcripts (`<config_dir>/projects/<slug>/<id>.jsonl`) the
/// real picker draws from.
#[tauri::command]
pub fn list_resumable_conversations(
    db: State<DbConnection>,
    session_id: String,
) -> Result<Vec<ResumableConversation>, String> {
    let (config_dir_path, project_path, current_provider_session_id): (String, String, Option<String>) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let (account_id, project_id, resume): (String, String, Option<String>) = conn
            .query_row(
                "SELECT account_id, project_id, provider_session_id FROM sessions WHERE id = ?1",
                params![session_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| format!("session lookup failed: {e}"))?;
        let config_dir_path: String = conn
            .query_row("SELECT config_dir_path FROM accounts WHERE id = ?1", params![account_id], |row| row.get(0))
            .map_err(|e| format!("account lookup failed: {e}"))?;
        let project_path: String = conn
            .query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
            .map_err(|e| format!("project lookup failed: {e}"))?;
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
