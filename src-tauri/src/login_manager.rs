use crate::providers::claude;
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

/// A running interactive CLI login. Kept alive in the registry so we can
/// write the pasted OAuth code to its stdin and kill it on cancel.
pub struct LoginProc {
    pub child: Box<dyn Child + Send + Sync>,
    pub writer: Box<dyn Write + Send>,
    pub config_dir: String,
    #[allow(dead_code)]
    pub provider: String,
}

pub struct LoginRegistry(pub Mutex<HashMap<String, LoginProc>>);

impl LoginRegistry {
    pub fn new() -> Self {
        LoginRegistry(Mutex::new(HashMap::new()))
    }
}

#[derive(Clone, Serialize)]
struct LoginOutputEvent {
    login_id: String,
    text: String,
}

#[derive(Clone, Serialize)]
struct LoginExitedEvent {
    login_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginSession {
    pub login_id: String,
    pub config_dir: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub logged_in: bool,
    pub auth_method: String,
}

/// Path of the isolated, not-yet-adopted config dir for a login attempt.
/// `create_account` later moves this into the account's permanent dir.
pub fn pending_dir(app: &AppHandle, login_id: &str) -> Result<std::path::PathBuf, String> {
    Ok(app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("pending-logins")
        .join(login_id))
}

/// Full parent environment plus an isolated `CLAUDE_CONFIG_DIR` and a sane
/// `TERM`, so the CLI renders and can find its own helpers regardless of how
/// Fleet itself was launched.
fn login_env(config_dir: &str) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = std::env::vars().collect();
    env.retain(|(k, _)| k != "CLAUDE_CONFIG_DIR");
    env.push(("CLAUDE_CONFIG_DIR".to_string(), config_dir.to_string()));
    if !env.iter().any(|(k, _)| k == "TERM") {
        env.push(("TERM".to_string(), "xterm-256color".to_string()));
    }
    env
}

pub fn begin_login(app: AppHandle, provider: String) -> Result<LoginSession, String> {
    if provider != "claude" {
        return Err(format!(
            "'{provider}' login isn't wired yet — its CLI isn't installed on this machine"
        ));
    }
    if !crate::providers::is_provider_available(&provider) {
        return Err("the claude CLI wasn't found — install Claude Code first".to_string());
    }

    let login_id = Uuid::new_v4().to_string();
    let config_dir = pending_dir(&app, &login_id)?;
    std::fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    let config_dir = config_dir.to_string_lossy().to_string();

    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize {
            rows: 30,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(claude::binary());
    for arg in claude::login_args() {
        cmd.arg(arg);
    }
    if let Ok(home) = std::env::var("HOME") {
        cmd.cwd(home);
    }
    for (k, v) in login_env(&config_dir) {
        cmd.env(k, v);
    }

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    app.state::<LoginRegistry>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .insert(
            login_id.clone(),
            LoginProc {
                child,
                writer,
                config_dir: config_dir.clone(),
                provider,
            },
        );

    // Stream raw output chunks (not line-buffered) so the "Paste code here >"
    // prompt — which has no trailing newline — shows up immediately.
    let stream_app = app.clone();
    let stream_id = login_id.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let text = String::from_utf8_lossy(&buf[..n]).to_string();
                    let _ = stream_app.emit(
                        "login-output",
                        LoginOutputEvent {
                            login_id: stream_id.clone(),
                            text,
                        },
                    );
                }
                Err(_) => break,
            }
        }
        let _ = stream_app.emit(
            "login-exited",
            LoginExitedEvent {
                login_id: stream_id.clone(),
            },
        );
    });

    Ok(LoginSession { login_id, config_dir })
}

/// Writes the pasted OAuth code (plus newline) to the login process's stdin.
pub fn send_login_input(app: &AppHandle, login_id: &str, mut data: String) -> Result<(), String> {
    if !data.ends_with('\n') {
        data.push('\n');
    }
    let state = app.state::<LoginRegistry>();
    let mut registry = state.0.lock().map_err(|e| e.to_string())?;
    let proc = registry
        .get_mut(login_id)
        .ok_or_else(|| "login session not found".to_string())?;
    proc.writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    proc.writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Runs `claude auth status --json` scoped to a config dir and parses it.
pub fn auth_status_for_dir(config_dir: &str) -> Result<AuthStatus, String> {
    let mut env = login_env(config_dir);
    // Never let an inherited ANTHROPIC_API_KEY mask the real login state.
    env.retain(|(k, _)| k != "ANTHROPIC_API_KEY");

    let mut cmd = Command::new(claude::binary());
    cmd.args(claude::auth_status_args());
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }

    let output = cmd.output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(stdout.trim())
        .map_err(|e| format!("could not parse auth status: {e} — got: {stdout}"))?;

    Ok(AuthStatus {
        logged_in: value.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false),
        auth_method: value
            .get("authMethod")
            .and_then(|v| v.as_str())
            .unwrap_or("none")
            .to_string(),
    })
}

pub fn check_login_status(app: &AppHandle, login_id: &str) -> Result<AuthStatus, String> {
    let config_dir = {
        let state = app.state::<LoginRegistry>();
        let registry = state.0.lock().map_err(|e| e.to_string())?;
        registry
            .get(login_id)
            .map(|p| p.config_dir.clone())
            .ok_or_else(|| "login session not found".to_string())?
    };
    auth_status_for_dir(&config_dir)
}

pub fn cancel_login(app: &AppHandle, login_id: &str) -> Result<(), String> {
    let state = app.state::<LoginRegistry>();
    let mut registry = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(mut proc) = registry.remove(login_id) {
        let _ = proc.child.kill();
    }
    if let Ok(dir) = pending_dir(app, login_id) {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(())
}

/// Removes a finished login from the registry (killing it if somehow still
/// alive). Called by `create_account` once it adopts the config dir.
pub fn take(app: &AppHandle, login_id: &str) {
    if let Some(state) = app.try_state::<LoginRegistry>() {
        if let Ok(mut registry) = state.0.lock() {
            if let Some(mut proc) = registry.remove(login_id) {
                let _ = proc.child.kill();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_env_overrides_config_dir_and_sets_term() {
        let env = login_env("/tmp/some-isolated-dir");
        let config_dir_entries: Vec<_> = env.iter().filter(|(k, _)| k == "CLAUDE_CONFIG_DIR").collect();
        assert_eq!(config_dir_entries.len(), 1, "must not carry a duplicate/stale CLAUDE_CONFIG_DIR");
        assert_eq!(config_dir_entries[0].1, "/tmp/some-isolated-dir");
        assert!(env.iter().any(|(k, _)| k == "TERM"));
    }

    /// Exercises the exact code path the app uses to check login state,
    /// against a real `claude` invocation (no mocking) and a scratch config
    /// dir that has never been logged into — this is the same command used
    /// by `check_account_auth`/AccountList, just without going through the
    /// Tauri IPC layer we can't drive headlessly.
    #[test]
    fn auth_status_for_dir_reports_logged_out_for_a_fresh_dir() {
        let dir = std::env::temp_dir().join(format!("fleet-test-auth-status-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let status = auth_status_for_dir(dir.to_str().unwrap()).expect("claude auth status should run and parse");
        assert!(!status.logged_in);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
