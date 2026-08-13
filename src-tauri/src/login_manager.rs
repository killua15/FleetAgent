use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::process::Command;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager, Runtime};
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

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AuthStatus {
    pub logged_in: bool,
    pub auth_method: String,
    /// Both come straight from `claude auth status --json`, which has always
    /// returned them — they were simply discarded. They're what makes a list
    /// of config directories readable: `~/.claude-work` means nothing on its
    /// own, `dsalinas@technomar.com.br · team` identifies the account.
    /// `None` when the CLI omits them (a team login without a bound email).
    pub email: Option<String>,
    pub subscription_type: Option<String>,
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

/// The permanent home for a new login, named after the account rather than an
/// opaque id: `<app data>/accounts/<slug>`. This *is* where `claude auth
/// login` runs, so it's also the config dir forever after — the directory
/// can't be renamed later without orphaning the keychain token (see
/// `create_account`). That's the whole reason the name has to be known before
/// the login starts, and why a new account no longer needs a manual "move it
/// somewhere readable" step afterwards.
///
/// `~/.<slug>` rather than somewhere under Hangar's own app-data directory —
/// every provider's CLI (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, ...) is happy
/// pointed anywhere, and a dot-directory in $HOME is the convention every one
/// of them already uses when run outside Hangar. AddAccount.tsx checks the
/// name against every existing account before this is ever called, so a
/// collision here means that check was bypassed (a race between two tabs, or
/// a stale account list) — refusing outright is correct, not disambiguating
/// with a random suffix the whole point of this change was to get away from.
fn account_config_dir(name: &str) -> Result<std::path::PathBuf, String> {
    let home = crate::app_data::home_dir().ok_or_else(|| "no home directory".to_string())?;
    account_dir_under(&home, name)
}

/// The testable half of `account_config_dir` — takes `home` as a plain
/// argument instead of reading it from the process environment, which is
/// global and shared with every other test running in parallel (same split
/// `app_data.rs` makes between `root` and `expand_home`).
fn account_dir_under(home: &str, name: &str) -> Result<std::path::PathBuf, String> {
    let slug = crate::commands::sessions::slugify(name);
    let candidate = std::path::PathBuf::from(home).join(format!(".{slug}"));
    if candidate.exists() {
        return Err(format!("{} already exists — pick a different account name", candidate.display()));
    }
    Ok(candidate)
}

/// Answers the CLI's first-run question before it gets asked.
///
/// A config directory that has never run interactively opens on a theme
/// picker — and since every account here gets its own directory, that menu is
/// the first thing a new account shows, in the login PTY, in the first
/// terminal, and in the quota scrape. It's not a preference anyone came here
/// to express, and a user who has never seen Claude Code reads it as Hangar
/// being broken.
///
/// The value written is exactly what the CLI writes itself when the menu is
/// answered — verified by answering it in a scratch directory and reading the
/// file back — so this isn't a guessed schema, it's the same file one step
/// earlier. Merged rather than overwritten, and silent on failure: an
/// unwritable settings file is a cosmetic problem, not a reason to refuse to
/// add an account.
fn seed_first_run_settings(config_dir: &std::path::Path) {
    let path = config_dir.join("settings.json");
    let mut settings: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    let Some(object) = settings.as_object_mut() else { return };
    if object.contains_key("theme") {
        return;
    }
    object.insert("theme".to_string(), serde_json::Value::String("dark".to_string()));
    if let Ok(pretty) = serde_json::to_string_pretty(&settings) {
        let _ = std::fs::write(&path, pretty);
    }
}

/// Full parent environment plus an isolated `CLAUDE_CONFIG_DIR` and a sane
/// `TERM`, so the CLI renders and can find its own helpers regardless of how
/// Hangar itself was launched. `pub(crate)` so `usage_scraper` (another
/// interactive-PTY consumer of the same CLI) can reuse it instead of
/// duplicating the same env-assembly logic.
pub(crate) fn login_env(config_dir: &str) -> Vec<(String, String)> {
    login_env_for("claude", config_dir)
}

/// Full parent environment plus the isolation variable **this provider** uses,
/// and a sane `TERM` so the CLI renders regardless of how Hangar was launched.
///
/// The variable is per-CLI and getting it wrong is worse than failing: a Codex
/// login run with `CLAUDE_CONFIG_DIR` set would write into the shared
/// `~/.codex`, and the account would look isolated while sharing credentials
/// with every other one.
pub(crate) fn login_env_for(provider: &str, config_dir: &str) -> Vec<(String, String)> {
    let var = crate::providers::spec(provider).map(|s| s.config_dir_env).unwrap_or("CLAUDE_CONFIG_DIR");
    let mut env: Vec<(String, String)> = std::env::vars().collect();
    env.retain(|(k, _)| k != var);
    env.push((var.to_string(), config_dir.to_string()));
    if !env.iter().any(|(k, _)| k == "TERM") {
        env.push(("TERM".to_string(), "xterm-256color".to_string()));
    }
    env
}

/// Starts an interactive `claude auth login` for a brand-new account.
///
/// `name` is the account name the user typed: the login runs directly in that
/// account's permanent home (`~/.<slug>`), so the account is correctly
/// configured the moment it's created — no later "move the config dir
/// somewhere readable" step. The name has to be known here, before the login,
/// precisely because the directory can't be renamed afterwards without
/// breaking the keychain token.
pub fn begin_login(app: AppHandle, provider: String, name: String) -> Result<LoginSession, String> {
    // Two different failures that used to be reported as the same one — and as
    // the wrong one. Saying "its CLI isn't installed" to someone who has it
    // installed sends them to fix a machine that isn't broken.
    let Some(spec) = crate::providers::spec(&provider) else {
        return Err(format!("Hangar doesn't know a provider called '{provider}'"));
    };
    let Some(login_args) = spec.login_args else {
        return Err(format!(
            "'{provider}' has no interactive login — it authenticates with an API key, so add it as a key-billed account instead"
        ));
    };
    if !crate::providers::is_provider_available(&provider) {
        return Err(format!("the `{}` CLI wasn't found on this machine — install it first", spec.binary));
    }

    let login_id = Uuid::new_v4().to_string();
    let config_dir = account_config_dir(&name)?;
    std::fs::create_dir_all(&config_dir).map_err(|e| e.to_string())?;
    seed_first_run_settings(&config_dir);
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

    let mut cmd = CommandBuilder::new(crate::providers::resolve_binary(spec.binary));
    for arg in login_args {
        cmd.arg(arg);
    }
    if let Some(home) = crate::app_data::home_dir() {
        cmd.cwd(home);
    }
    for (k, v) in login_env_for(&provider, &config_dir) {
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
    auth_status_for("claude", config_dir)
}

/// Asks **this provider's** CLI whether the config dir is logged in.
///
/// It used to run `claude auth status --json` no matter what, which is why a
/// Codex login hung forever: the user authorised in the browser, Codex wrote
/// its credentials, and Hangar then asked *Claude* about a directory Claude had
/// never seen. It answered "logged out", correctly, and the wizard waited for a
/// state that could never arrive.
pub fn auth_status_for(provider: &str, config_dir: &str) -> Result<AuthStatus, String> {
    auth_status_with(provider, config_dir, TrustMarkerFile::Yes)
}

/// Whether the on-disk credential file counts as proof on its own.
///
/// It does while a login is in progress — the file appearing *is* the login
/// finishing, and for a CLI with no status probe that's the only signal there
/// is. It does not when the question is "is this credential still valid": an
/// expired token is still a file on disk.
///
/// **What this buys, measured rather than assumed.** Against the installed
/// CLIs, with a bogus credential planted in a scratch config dir:
///
/// - `claude auth status --json` answers `loggedIn: false`. Real validation, so
///   asking it is what lets an expired Claude account be noticed at all.
/// - `codex login status` answers "Logged in using ChatGPT" for any `auth.json`
///   that merely exists. Its probe is no stronger than the marker file, so
///   bypassing the marker changes nothing for Codex — an expired-but-present
///   Codex credential cannot be detected by anything Hangar can ask today.
///
/// The split is still right: it's what makes the Claude case work, and it puts
/// the seam in the correct place for the day Codex's CLI learns to answer
/// honestly. It just isn't a fix for Codex, and shouldn't be described as one.
#[derive(Clone, Copy, PartialEq)]
pub enum TrustMarkerFile {
    Yes,
    /// Ask the CLI, whatever is on disk.
    No,
}

pub fn auth_status_with(provider: &str, config_dir: &str, trust_marker: TrustMarkerFile) -> Result<AuthStatus, String> {
    let Some(spec) = crate::providers::spec(provider) else {
        return Err(format!("Hangar doesn't know a provider called '{provider}'"));
    };

    // On disk before on the CLI: it's free, it can't time out, and it's what
    // the credential actually *is* rather than a sentence about it.
    if trust_marker == TrustMarkerFile::Yes {
        if let Some(marker) = crate::providers::auth_marker_file(provider) {
            if std::path::Path::new(config_dir).join(marker).is_file() {
                return Ok(AuthStatus { logged_in: true, auth_method: "oauth".to_string(), email: None, subscription_type: None });
            }
        }
    }

    let args: Vec<&str> = match spec.auth_status {
        crate::providers::AuthStatusProbe::Json(args) | crate::providers::AuthStatusProbe::Text { args, .. } => args.to_vec(),
        // Nothing to ask: a key-billed provider is authenticated when it has a
        // key, and the wizard validates that separately.
        crate::providers::AuthStatusProbe::None => {
            return Ok(AuthStatus { logged_in: false, auth_method: "key".to_string(), email: None, subscription_type: None })
        }
    };

    let mut env = login_env_for(provider, config_dir);
    // Never let an inherited key mask the real login state.
    env.retain(|(k, _)| k != spec.key_env);

    let mut cmd = Command::new(crate::providers::resolve_binary(spec.binary));
    cmd.args(&args);
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }

    let output = crate::process_timeout::run_with_timeout(cmd, crate::process_timeout::DEFAULT_TIMEOUT)?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    match spec.auth_status {
        crate::providers::AuthStatusProbe::Json(_) => {
            let value: serde_json::Value = serde_json::from_str(stdout.trim())
                .map_err(|e| format!("could not parse auth status: {e} — got: {stdout}"))?;
            Ok(AuthStatus {
                logged_in: value.get("loggedIn").and_then(|v| v.as_bool()).unwrap_or(false),
                auth_method: value.get("authMethod").and_then(|v| v.as_str()).unwrap_or("none").to_string(),
                email: value.get("email").and_then(|v| v.as_str()).map(str::to_string),
                subscription_type: value.get("subscriptionType").and_then(|v| v.as_str()).map(str::to_string),
            })
        }
        // Coarser by necessity: the CLI prints a sentence, so there's no email
        // or plan to report and the UI shows the name the user typed rather
        // than inventing detail it doesn't have.
        crate::providers::AuthStatusProbe::Text { logged_marker, .. } => Ok(AuthStatus {
            logged_in: stdout.contains(logged_marker),
            auth_method: stdout.trim().to_string(),
            email: None,
            subscription_type: None,
        }),
        crate::providers::AuthStatusProbe::None => unreachable!("handled above"),
    }
}

pub fn check_login_status<R: Runtime>(app: &AppHandle<R>, login_id: &str) -> Result<AuthStatus, String> {
    let (config_dir, provider) = {
        let state = app.state::<LoginRegistry>();
        let registry = state.0.lock().map_err(|e| e.to_string())?;
        registry
            .get(login_id)
            .map(|p| (p.config_dir.clone(), p.provider.clone()))
            .ok_or_else(|| "login session not found".to_string())?
    };

    // A provider with a real interactive login but `AuthStatusProbe::None`
    // (Gemini, and Grok inherited it — see the ASSUMPTION comment on its
    // `spec()` entry) has no CLI command to ask, and `auth_status_for` below
    // answers `logged_in: false` unconditionally for `None`. Polled forever,
    // that's not "not logged in yet" — it's a wizard that can never advance
    // past step 3 no matter what the user does in the browser. The best
    // signal available without knowing that CLI's real status output is
    // whether the login process itself exited cleanly — worse than a real
    // probe (nothing here can tell "succeeded" from "closed the window right
    // as the OAuth redirect landed"), but strictly better than a permanent
    // deadlock. Swap this for a real probe or marker file the day either
    // provider's actual output is known.
    if let Some(spec) = crate::providers::spec(&provider) {
        if spec.login_args.is_some() && matches!(spec.auth_status, crate::providers::AuthStatusProbe::None) {
            let logged_in = {
                let state = app.state::<LoginRegistry>();
                let mut registry = state.0.lock().map_err(|e| e.to_string())?;
                registry.get_mut(login_id).and_then(|p| p.child.try_wait().ok().flatten()).is_some_and(|status| status.success())
            };
            return Ok(AuthStatus { logged_in, auth_method: "exit-code".to_string(), email: None, subscription_type: None });
        }
    }

    // The provider comes off the login itself — asking the wrong CLI is the bug
    // that made a completed Codex login look like it never happened.
    auth_status_for(&provider, &config_dir)
}

pub fn cancel_login(app: &AppHandle, login_id: &str) -> Result<(), String> {
    let state = app.state::<LoginRegistry>();
    let mut registry = state.0.lock().map_err(|e| e.to_string())?;
    // The login's dir is now `~/.<slug>` (see `begin_login`), not a
    // reconstructable `pending-logins/<id>` — so take the real path off the
    // registered proc before removing it, rather than guessing where it was.
    if let Some(mut proc) = registry.remove(login_id) {
        let _ = proc.child.kill();
        let _ = std::fs::remove_dir_all(&proc.config_dir);
    }
    Ok(())
}

/// Kills every in-flight interactive login, used on app exit. These are real
/// `claude auth login` processes sitting in a PTY waiting on stdin — quitting
/// Hangar with the AddAccount wizard open would otherwise strand one forever.
/// Their pending config dirs are deliberately left on disk: an OAuth login
/// that actually completed before the quit is still adoptable next launch.
pub fn kill_all_logins<R: tauri::Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<LoginRegistry>() else { return };
    let Ok(mut registry) = state.0.lock() else { return };
    for (login_id, mut proc) in registry.drain() {
        log::info!("killing in-flight login {login_id} on exit");
        let _ = proc.child.kill();
    }
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

/// Like `take`, but returns the login's real config dir so the caller doesn't
/// have to reconstruct it. Since `begin_login` now writes to `~/.<slug>`
/// rather than a path derivable from the login id, `create_account` must adopt
/// whatever path was actually used, and only the registered proc knows it.
/// `None` if the login id isn't registered (already adopted, or cancelled).
pub fn adopt(app: &AppHandle, login_id: &str) -> Option<String> {
    let state = app.try_state::<LoginRegistry>()?;
    let mut registry = state.0.lock().ok()?;
    let mut proc = registry.remove(login_id)?;
    let _ = proc.child.kill();
    Some(proc.config_dir)
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

    /// A new login lands in a readable, account-named dot-directory directly
    /// under the home Hangar was given — the same convention every CLI here
    /// already uses when someone runs its login by hand outside Hangar. The
    /// name is slugified, so the resulting path is filesystem-safe regardless
    /// of what was typed.
    #[test]
    fn a_new_login_dir_is_a_home_dotdir_named_after_the_account() {
        let home = std::env::temp_dir().join(format!("hangar-test-login-home-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let dir = account_dir_under(home.to_str().unwrap(), "David Personal 1").unwrap();

        assert_eq!(dir.parent().unwrap(), home, "must live directly under home, not some nested accounts/ folder");
        assert_eq!(dir.file_name().unwrap().to_string_lossy(), ".david-personal-1", "the folder must be a dot-dir named after the slugified account");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// Two accounts can't share one config dir — that would point both at the
    /// same login. `AddAccount.tsx` is supposed to catch this before it ever
    /// reaches here (checked against every existing account's name), so
    /// getting here at all means that check was bypassed — refuse outright
    /// rather than silently landing the second account somewhere else.
    #[test]
    fn a_colliding_account_name_is_rejected() {
        let home = std::env::temp_dir().join(format!("hangar-test-collide-home-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        let first = account_dir_under(home.to_str().unwrap(), "Personal").unwrap();
        std::fs::create_dir_all(&first).unwrap();

        let err = account_dir_under(home.to_str().unwrap(), "Personal").unwrap_err();
        assert!(err.contains("already exists"), "must refuse rather than disambiguate: {err}");

        let _ = std::fs::remove_dir_all(&home);
    }

    /// The signal that doesn't depend on parsing a sentence: Codex writes
    /// `auth.json` into its config dir, and every tool that switches Codex
    /// accounts works by moving that file.
    ///
    /// Needs a real `codex` on the machine despite the name: the half of this
    /// that asserts the marker file is enough never spawns anything, but the
    /// empty-dir half has no marker to read and so falls through to the CLI —
    /// which on a machine without it fails to start rather than answering.
    #[test]
    #[ignore = "needs the real `codex` CLI installed — see doc comment"]
    fn a_codex_dir_with_credentials_reads_as_logged_in_without_spawning_anything() {
        let dir = std::env::temp_dir().join(format!("hangar-test-codex-auth-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.to_str().unwrap();

        // Empty dir: whatever the CLI says, it must not claim to be logged in.
        let empty = auth_status_for("codex", path).expect("an empty codex home must still answer");
        assert!(!empty.logged_in);

        std::fs::write(dir.join("auth.json"), "{}").unwrap();
        let filled = auth_status_for("codex", path).expect("the marker file alone is enough");
        assert!(filled.logged_in, "a codex home holding auth.json is logged in");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Gemini (and Grok, which copied its shape) has a real interactive login
    /// but `AuthStatusProbe::None` — no CLI command to ask "am I logged in?".
    /// Before the exit-code fallback, `check_login_status` answered `false`
    /// unconditionally for these, forever, even after the user finished the
    /// browser OAuth flow: a permanent deadlock in the wizard, not a slow
    /// poll. `true` stands in for a real login CLI here — this only needs
    /// something that actually exits 0, not gemini/grok installed.
    #[test]
    fn a_login_with_no_status_probe_reads_as_logged_in_once_its_process_exits_cleanly() {
        let app = tauri::test::mock_app();
        app.manage(LoginRegistry::new());
        let handle = app.handle().clone();

        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        let child = pair.slave.spawn_command(CommandBuilder::new("true")).unwrap();
        drop(pair.slave);
        let writer = pair.master.take_writer().unwrap();

        let login_id = "test-login".to_string();
        handle.state::<LoginRegistry>().0.lock().unwrap().insert(
            login_id.clone(),
            LoginProc { child, writer, config_dir: "/tmp/irrelevant-for-this-test".to_string(), provider: "gemini".to_string() },
        );

        // `try_wait` is non-blocking and `true` exits almost immediately, but
        // "almost" is still a race against a fresh process — poll instead of
        // asserting on the first check.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut status = check_login_status(&handle, &login_id).unwrap();
        while !status.logged_in && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
            status = check_login_status(&handle, &login_id).unwrap();
        }

        assert!(status.logged_in, "a cleanly-exited login process must read as logged in when there is no real probe to ask instead");
        assert_eq!(status.auth_method, "exit-code");
    }

    /// A provider that never finishes (still running, or was killed) must not
    /// be reported as logged in just because nothing has happened yet — the
    /// fallback is "exited successfully", not "not exited unsuccessfully".
    #[test]
    fn a_login_with_no_status_probe_that_is_still_running_is_not_logged_in() {
        let app = tauri::test::mock_app();
        app.manage(LoginRegistry::new());
        let handle = app.handle().clone();

        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        // `sleep` stands in for a login CLI still waiting on the browser —
        // still running when this test asserts against it.
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("5");
        let child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let writer = pair.master.take_writer().unwrap();

        let login_id = "test-login-still-running".to_string();
        handle.state::<LoginRegistry>().0.lock().unwrap().insert(
            login_id.clone(),
            LoginProc { child, writer, config_dir: "/tmp/irrelevant-for-this-test".to_string(), provider: "gemini".to_string() },
        );

        let status = check_login_status(&handle, &login_id).unwrap();
        assert!(!status.logged_in, "a login process that hasn't exited yet must not read as logged in");

        // Don't leave a live sleep behind for the next test to trip over.
        handle.state::<LoginRegistry>().0.lock().unwrap().remove(&login_id).unwrap().child.kill().ok();
    }

    /// Exercises the exact code path the app uses to check login state,
    /// against a real `claude` invocation (no mocking) and a scratch config
    /// dir that has never been logged into.
    ///
    /// `#[ignore]` because it needs Claude Code actually installed: it's a
    /// genuinely valuable integration test on a dev machine, but on CI it
    /// would only ever assert that the runner has no `claude` binary. Run it
    /// with `cargo test -- --ignored`.
    /// The theme menu is the first thing a brand-new account shows — in the
    /// login PTY, in its first terminal, and in the quota scrape. Writing the
    /// same value the CLI itself writes means nobody is ever asked.
    #[test]
    fn a_new_config_dir_is_seeded_so_the_theme_menu_never_appears() {
        let dir = std::env::temp_dir().join(format!("hangar-test-seed-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        seed_first_run_settings(&dir);
        let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(written.get("theme").and_then(|t| t.as_str()), Some("dark"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A directory that already has settings must keep them — this runs on a
    /// path the user may have pointed at an existing Claude installation.
    #[test]
    fn seeding_never_overwrites_settings_that_are_already_there() {
        let dir = std::env::temp_dir().join(format!("hangar-test-seed-keep-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), r#"{"theme":"light","model":"opus"}"#).unwrap();

        seed_first_run_settings(&dir);
        let kept: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(dir.join("settings.json")).unwrap()).unwrap();
        assert_eq!(kept.get("theme").and_then(|t| t.as_str()), Some("light"), "a chosen theme is not ours to change");
        assert_eq!(kept.get("model").and_then(|m| m.as_str()), Some("opus"), "and everything else has to survive");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore = "requires a real `claude` CLI installed on the machine"]
    fn auth_status_for_dir_reports_logged_out_for_a_fresh_dir() {
        let dir = std::env::temp_dir().join(format!("hangar-test-auth-status-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let status = auth_status_for_dir(dir.to_str().unwrap()).expect("claude auth status should run and parse");
        assert!(!status.logged_in);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
