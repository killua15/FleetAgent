use crate::db::DbConnection;
use crate::providers;
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use rusqlite::params;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use uuid::Uuid;

pub struct ProcessRegistry(pub Mutex<HashMap<String, Box<dyn Child + Send + Sync>>>);

impl ProcessRegistry {
    pub fn new() -> Self {
        ProcessRegistry(Mutex::new(HashMap::new()))
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionOutputEvent {
    pub session_id: String,
    pub kind: String,
    pub text: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountExhaustedEvent {
    pub account_id: String,
    pub session_id: String,
}

/// Shared by process_manager (raw PTY output) and turn_runner (structured
/// stream-json turns) so both paths persist/broadcast blocks identically.
/// Generic over `Runtime` so tests can drive it with `tauri::test::mock_app()`
/// instead of only the real Wry-backed app. Returns the persisted block's id
/// so callers that produced it from a tool-use event (turn_runner) can link
/// a `file_edits` row back to the exact block that caused it.
pub(crate) fn emit_block<R: Runtime>(app: &AppHandle<R>, session_id: &str, kind: &str, text: &str) -> String {
    let event = SessionOutputEvent {
        session_id: session_id.to_string(),
        kind: kind.to_string(),
        text: text.to_string(),
    };
    let _ = app.emit("session-output", event);

    let block_id = Uuid::new_v4().to_string();
    match app.try_state::<DbConnection>() {
        Some(db) => match db.0.lock() {
            Ok(conn) => {
                if let Err(e) = conn.execute(
                    "INSERT INTO session_blocks (id, session_id, kind, text, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![block_id, session_id, kind, text, chrono::Utc::now().to_rfc3339()],
                ) {
                    eprintln!("emit_block: failed to persist block for session {session_id}: {e}");
                }
            }
            Err(e) => eprintln!("emit_block: DB mutex poisoned, block for session {session_id} lost: {e}"),
        },
        None => eprintln!("emit_block: DbConnection not managed yet, block for session {session_id} lost"),
    }
    block_id
}

pub(crate) fn mark_session_status<R: Runtime>(app: &AppHandle<R>, session_id: &str, status: &str) {
    let mut session_name = None;
    if let Some(db) = app.try_state::<DbConnection>() {
        match db.0.lock() {
            Ok(conn) => {
                let result = if status == "done" {
                    conn.execute(
                        "UPDATE sessions SET status = ?2, ended_at = ?3 WHERE id = ?1",
                        params![session_id, status, chrono::Utc::now().to_rfc3339()],
                    )
                } else {
                    conn.execute("UPDATE sessions SET status = ?2 WHERE id = ?1", params![session_id, status])
                };
                if let Err(e) = result {
                    eprintln!("mark_session_status: failed to set session {session_id} to {status}: {e}");
                }
                session_name = conn.query_row("SELECT name FROM sessions WHERE id = ?1", params![session_id], |row| row.get::<_, String>(0)).ok();
            }
            Err(e) => eprintln!("mark_session_status: DB mutex poisoned for session {session_id}: {e}"),
        }
    }
    let _ = app.emit("session-status", (session_id.to_string(), status.to_string()));

    // Every real "waiting" transition comes from turn_runner at the tail of
    // a turn (spawn failure, non-claude provider bailout, or a clean finish)
    // — never at session creation, which sets the row's initial status
    // directly via SQL — so there's no spurious case to filter here.
    if status == "waiting" {
        let name = session_name.unwrap_or_else(|| "A session".to_string());
        crate::notifications::notify_if_enabled(app, |s| s.notify_session_waiting, "Session finished", &format!("{name} is waiting for you"));
    }
}

pub(crate) fn mark_account_exhausted<R: Runtime>(app: &AppHandle<R>, account_id: &str) {
    if let Some(db) = app.try_state::<DbConnection>() {
        match db.0.lock() {
            Ok(conn) => {
                if let Err(e) = conn.execute(
                    "UPDATE accounts SET status = 'exhausted', updated_at = ?2 WHERE id = ?1",
                    params![account_id, chrono::Utc::now().to_rfc3339()],
                ) {
                    eprintln!("mark_account_exhausted: failed to update account {account_id}: {e}");
                }
            }
            Err(e) => eprintln!("mark_account_exhausted: DB mutex poisoned for account {account_id}: {e}"),
        }
    }
}

pub(crate) fn emit_account_exhausted<R: Runtime>(app: &AppHandle<R>, account_id: &str, session_id: &str) {
    mark_account_exhausted(app, account_id);
    let _ = app.emit(
        "account-exhausted",
        AccountExhaustedEvent {
            account_id: account_id.to_string(),
            session_id: session_id.to_string(),
        },
    );

    let account_name = app
        .try_state::<DbConnection>()
        .and_then(|db| db.0.lock().ok().and_then(|conn| conn.query_row("SELECT name FROM accounts WHERE id = ?1", params![account_id], |row| row.get::<_, String>(0)).ok()))
        .unwrap_or_else(|| "An account".to_string());
    crate::notifications::notify_if_enabled(
        app,
        |s| s.notify_account_exhausted,
        "Account rate-limited",
        &format!("{account_name} hit a rate limit and can't run turns until it clears"),
    );
}

/// Generic interactive-PTY spawner. Not used by the headless turn-based
/// AgentPanel flow (see turn_runner) — kept for the embedded-terminal CLI
/// login step in the AddAccount wizard, which needs a real TTY.
#[allow(dead_code)]
pub fn spawn(
    app: AppHandle,
    session_id: String,
    provider: String,
    account_id: String,
    program: &str,
    args: &[String],
    cwd: &str,
    env: &[(String, String)],
) -> Result<u32, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 40,
            cols: 160,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(program);
    for a in args {
        cmd.arg(a);
    }
    cmd.cwd(cwd);
    for (k, v) in env {
        cmd.env(k, v);
    }

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    let pid = child.process_id().unwrap_or(0);
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;

    app.state::<ProcessRegistry>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .insert(session_id.clone(), child);

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut acc = String::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    acc.push_str(&String::from_utf8_lossy(&buf[..n]));
                    while let Some(pos) = acc.find('\n') {
                        let line: String = acc.drain(..=pos).collect();
                        let line = line.trim_end_matches(['\r', '\n']).to_string();
                        if !line.is_empty() {
                            handle_line(&app, &session_id, &provider, &account_id, &line);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if !acc.is_empty() {
            handle_line(&app, &session_id, &provider, &account_id, &acc);
        }
        mark_session_status(&app, &session_id, "done");
        let _ = app.emit("session-ended", session_id.to_string());
    });

    Ok(pid)
}

#[allow(dead_code)]
fn handle_line(app: &AppHandle, session_id: &str, provider: &str, account_id: &str, line: &str) {
    emit_block(app, session_id, "out", line);

    if providers::is_exhaustion_signal(provider, line) {
        emit_account_exhausted(app, account_id, session_id);
    }
}

pub fn kill<R: Runtime>(app: &AppHandle<R>, session_id: &str) -> Result<(), String> {
    let state = app.state::<ProcessRegistry>();
    let mut registry = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(mut child) = registry.remove(session_id) {
        child.kill().map_err(|e| e.to_string())?;
    }
    Ok(())
}
