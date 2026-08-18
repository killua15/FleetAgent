use crate::commands::sessions::{cleanup_worktree_rows, WorktreeCleanupRow};
use crate::credential_vault;
use crate::db::DbConnection;
use crate::login_manager;
use crate::models::{Account, ModelOption, NewAccount};
use crate::providers;
use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use serde_json::Value;
use std::process::Command;
use tauri::{AppHandle, Manager, Runtime, State};
use uuid::Uuid;

pub(crate) fn row_to_account(row: &rusqlite::Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get("id")?,
        provider: row.get("provider")?,
        billing: row.get("billing")?,
        name: row.get("name")?,
        scope: row.get("scope")?,
        plan_label: row.get("plan_label")?,
        config_dir_path: row.get("config_dir_path")?,
        credential_ref: row.get("credential_ref")?,
        monthly_cap: row.get("monthly_cap")?,
        status: row.get("status")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

#[tauri::command]
pub fn list_accounts(db: State<DbConnection>) -> Result<Vec<Account>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT * FROM accounts ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], row_to_account)
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct ProviderAvailability {
    pub provider: String,
    pub available: bool,
    /// Whether Hangar can run this one as a specific account at all. Separate
    /// from `available`, and the two fail differently: a missing CLI is
    /// something the user can go and install, this isn't.
    pub supported: bool,
}

/// Lets the wizard's step 1 flag providers whose CLI isn't installed, rather
/// than letting the user pick one and only discovering it's unusable later.
///
/// `supported` closes the other half of that same hole. Installing the CLI was
/// never the only way to reach a dead end: picking a provider Hangar can't
/// scope to let someone finish an interactive login and *then* find the account
/// greyed out in the terminal picker, which is a worse place to learn it.
#[tauri::command]
pub fn list_available_providers() -> Vec<ProviderAvailability> {
    ["claude", "codex", "gemini", "kimi", "grok"]
        .into_iter()
        .map(|p| ProviderAvailability {
            provider: p.to_string(),
            available: providers::is_provider_available(p),
            supported: providers::can_scope_to_account(p),
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyCheck {
    pub valid: bool,
    pub message: String,
}

/// Verifies an API key against the real provider endpoint before an account
/// is ever created — no more accepting "length > 6" as proof of a working key.
#[tauri::command]
pub async fn validate_api_key(provider: String, api_key: String) -> Result<ApiKeyCheck, String> {
    crate::blocking::run_blocking(move || {
        if provider != "claude" {
            return Ok(ApiKeyCheck {
                valid: false,
                message: format!("'{provider}' key validation isn't wired yet"),
            });
        }
        if api_key.trim().is_empty() {
            return Ok(ApiKeyCheck { valid: false, message: "empty key".to_string() });
        }

        let output = Command::new("curl")
            .args([
                "-s",
                "-o",
                "/dev/null",
                "-w",
                "%{http_code}",
                "--max-time",
                "10",
                "https://api.anthropic.com/v1/models",
                "-H",
                "anthropic-version: 2023-06-01",
                "-H",
            ])
            .arg(format!("x-api-key: {}", api_key.trim()))
            .output()
            .map_err(|e| format!("could not reach api.anthropic.com: {e}"))?;

        let code = String::from_utf8_lossy(&output.stdout).trim().to_string();
        match code.as_str() {
            "200" => Ok(ApiKeyCheck { valid: true, message: "Key verified".to_string() }),
            "401" => Ok(ApiKeyCheck { valid: false, message: "Invalid API key".to_string() }),
            "" => Ok(ApiKeyCheck { valid: false, message: "Network error reaching Anthropic".to_string() }),
            other => Ok(ApiKeyCheck { valid: false, message: format!("Unexpected response ({other})") }),
        }
    })
    .await?
}

/// Live model catalog for a given account. Key-billed accounts get the real
/// `/v1/models` list from Anthropic (genuinely "fetched via the SDK/API", not
/// hardcoded); subscription accounts have no raw API key to call that REST
/// endpoint with, so they fall back to the Claude Code CLI's own known
/// `--model` aliases — same fallback used if the live fetch fails for any
/// reason (network error, unexpected response shape, etc).
#[tauri::command]
pub async fn list_provider_models(app: AppHandle, account_id: String) -> Result<Vec<ModelOption>, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (provider, billing): (String, String) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT provider, billing FROM accounts WHERE id = ?1",
                params![account_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| e.to_string())?
        };

        if provider != "claude" {
            return Ok(vec![]);
        }

        if billing == "key" {
            if let Ok(key) = credential_vault::get(&account_id) {
                if let Some(models) = fetch_claude_models_from_api(&key) {
                    return Ok(models);
                }
            }
        }

        Ok(providers::claude::known_model_aliases())
    })
    .await?
}

fn fetch_claude_models_from_api(api_key: &str) -> Option<Vec<ModelOption>> {
    let output = Command::new("curl")
        .args([
            "-s",
            "--max-time",
            "10",
            "https://api.anthropic.com/v1/models",
            "-H",
            "anthropic-version: 2023-06-01",
            "-H",
        ])
        .arg(format!("x-api-key: {api_key}"))
        .output()
        .ok()?;

    let body = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(&body).ok()?;
    let data = value.get("data")?.as_array()?;

    let models: Vec<ModelOption> = data
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let label = m.get("display_name").and_then(|v| v.as_str()).unwrap_or(&id).to_string();
            Some(ModelOption { id, label })
        })
        .collect();

    if models.is_empty() {
        None
    } else {
        Some(models)
    }
}

#[tauri::command]
pub fn create_account(
    app: AppHandle,
    db: State<DbConnection>,
    account: NewAccount,
) -> Result<Account, String> {
    let id = Uuid::new_v4().to_string();

    if let Some(secret) = &account.secret {
        credential_vault::store(&id, secret)?;
    }

    let config_dir_path = if let Some(existing) = account.config_dir_path.as_deref().filter(|p| !p.trim().is_empty()) {
        // Adopting a directory the user already logged into by hand. Used
        // as-is for the same reason a pending-login dir is (see below): the
        // keychain entry is keyed by this exact path.
        let path = std::path::PathBuf::from(existing);
        if !path.is_dir() {
            return Err(format!("{existing} isn't a directory"));
        }
        path
    } else if let Some(login_id) = &account.login_id {
        // Adopt the login's already-authenticated config dir exactly where
        // `begin_login` ran it (`~/.<slug>`) — this *is* the account's
        // credential. Claude Code's OAuth token lives in the OS keychain,
        // keyed by the literal CLAUDE_CONFIG_DIR path used during
        // `claude auth login`, so the permanent config dir must stay exactly
        // there. `adopt` returns that real path from the registry rather than
        // reconstructing it, since it's no longer derivable from the id.
        let adopted = login_manager::adopt(&app, login_id).ok_or_else(|| format!("pending login {login_id} not found or already adopted"))?;
        let path = std::path::PathBuf::from(adopted);
        if !path.is_dir() {
            return Err(format!("login {login_id}'s config dir is missing"));
        }
        path
    } else {
        // Key-billed accounts (no interactive login, nothing imported) — same
        // `~/.<slug>` convention as a fresh CLI login (`login_manager.rs`'s
        // `account_config_dir`), so every provider's accounts live in the
        // same place regardless of how they authenticate. The frontend
        // checks the name against every existing account before this is
        // ever called; a collision here means that check was bypassed.
        let home = crate::app_data::home_dir().ok_or_else(|| "no home directory".to_string())?;
        let slug = crate::commands::sessions::slugify(&account.name);
        let config_dir_path = std::path::PathBuf::from(home).join(format!(".{slug}"));
        if config_dir_path.exists() {
            return Err(format!("{} already exists — pick a different account name", config_dir_path.display()));
        }
        std::fs::create_dir_all(&config_dir_path).map_err(|e| e.to_string())?;
        config_dir_path
    };
    let config_dir_path = config_dir_path.to_string_lossy().to_string();

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let now = Utc::now().to_rfc3339();

    conn.execute(
        "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, monthly_cap, status, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?1, ?8, 'healthy', ?9, ?9)",
        params![
            id,
            account.provider,
            account.billing,
            account.name,
            account.scope,
            account.plan_label,
            config_dir_path,
            account.monthly_cap,
            now,
        ],
    )
    .map_err(|e| e.to_string())?;

    conn.query_row("SELECT * FROM accounts WHERE id = ?1", params![id], row_to_account)
        .map_err(|e| e.to_string())
}

/// Re-runs the same check `login_manager` uses during login, scoped to an
/// already-created account — so AccountList can show real "logged in" state
/// instead of trusting a snapshot from creation time.
/// Off the UI thread: `auth_status_for` falls back to running the provider's
/// CLI when there's no marker file on disk, and AccountList asks this once per
/// account as it mounts.
#[tauri::command]
pub async fn check_account_auth(app: AppHandle, id: String) -> Result<login_manager::AuthStatus, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let (provider, billing, config_dir_path): (String, String, String) = conn
            .query_row(
                "SELECT provider, billing, config_dir_path FROM accounts WHERE id = ?1",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        drop(conn);

        if billing == "key" {
            return Ok(login_manager::AuthStatus { logged_in: true, auth_method: "api_key".to_string(), email: None, subscription_type: None });
        }
        // "Can this provider be asked at all", read from its spec — the same
        // question `list_account_auth` asks. It used to be hardcoded to Claude,
        // which reported every healthy Codex account as signed out: false, and
        // now visibly contradicted by the re-login banner, which knows better.
        let probeable = crate::providers::can_scope_to_account(&provider);
        if !probeable {
            return Ok(login_manager::AuthStatus { logged_in: false, auth_method: "unsupported".to_string(), email: None, subscription_type: None });
        }
        login_manager::auth_status_for(&provider, &config_dir_path)
    })
    .await?
}

/// A Claude Code config directory already on disk with a working login, that
/// no Hangar account is using yet.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredLogin {
    pub path: String,
    /// `~`-relative when it sits under the home directory, which is where all
    /// of these live in practice — the full path is noise in a list.
    pub display_path: String,
    pub email: Option<String>,
    pub subscription_type: Option<String>,
}

/// Claude Code config directories already logged in on this machine.
///
/// Hangar's own wizard parks each login in `pending-logins/<uuid>` and adopts
/// that as the account's permanent home (see `create_account`), which is fine
/// in isolation but ignores the directories a user already drives by hand —
/// `CLAUDE_CONFIG_DIR=~/.claude-work claude` and friends. Those are the ones
/// they know by name, so this finds them and lets an account point at one.
///
/// Adopting a directory in place is safe precisely because nothing moves: the
/// OAuth token lives in the OS keychain keyed by the literal path (see
/// `create_account`'s comment on why moving one orphans it).
///
/// Same discover-on-disk-then-import shape as
/// `commands::settings::list_discoverable_memory_files`.
#[tauri::command]
pub async fn list_discoverable_logins<R: Runtime>(app: AppHandle<R>) -> Result<Vec<DiscoveredLogin>, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let taken: Vec<String> = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare("SELECT config_dir_path FROM accounts").map_err(|e| e.to_string())?;
            let rows = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
        };

        let home = crate::app_data::home_dir().ok_or_else(|| "no home directory".to_string())?;
        let mut candidates: Vec<std::path::PathBuf> = std::fs::read_dir(&home)
            .map_err(|e| format!("could not read the home directory: {e}"))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                // `.claude` and `.claude-<whatever>`; a config dir always has a
                // `.claude.json` in it, which also rules out `.claude.json`
                // itself and the stray `.claude.json.backup` sitting alongside.
                path.is_dir()
                    && path.file_name().and_then(|n| n.to_str()).is_some_and(|name| name == ".claude" || name.starts_with(".claude-"))
                    && path.join(".claude.json").is_file()
            })
            .filter(|path| !taken.iter().any(|t| t == &path.to_string_lossy()))
            .collect();
        candidates.sort();

        // One thread each: `auth_status_for_dir` spawns a real CLI process
        // with a 25-second ceiling, so checking five directories in sequence
        // would be a two-minute worst case.
        let handles: Vec<_> = candidates
            .into_iter()
            .map(|path| {
                let home = home.clone();
                std::thread::spawn(move || {
                    let path_str = path.to_string_lossy().to_string();
                    // Literal on purpose: this scan looks for `~/.claude*`
                    // directories containing `.claude.json`, so it can only
                    // ever be about Claude.
                    let status = login_manager::auth_status_for("claude", &path_str).ok()?;
                    if !status.logged_in {
                        return None;
                    }
                    let display_path = path_str.strip_prefix(&home).map(|rest| format!("~{rest}")).unwrap_or_else(|| path_str.clone());
                    Some(DiscoveredLogin { path: path_str, display_path, email: status.email, subscription_type: status.subscription_type })
                })
            })
            .collect();

        Ok(handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect())
    })
    .await?
}

/// Repoints an account at a different config directory — the one fix for an
/// account created through the wizard that should have been using a login the
/// user already had.
///
/// Nothing is moved or copied: the directory stays exactly where its keychain
/// entry expects it. Repointing rather than delete-and-recreate is deliberate,
/// since `accounts` cascades to `sessions`, so recreating an account would
/// take its whole history — transcripts, worktrees, PRs — with it.
#[tauri::command]
pub async fn set_account_config_dir<R: Runtime>(app: AppHandle<R>, id: String, config_dir_path: String) -> Result<Account, String> {
    crate::blocking::run_blocking(move || {
        if !std::path::Path::new(&config_dir_path).is_dir() {
            return Err(format!("{config_dir_path} isn't a directory"));
        }
        let db = app.state::<DbConnection>();
        let provider: String = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT provider FROM accounts WHERE id = ?1", params![id], |row| row.get(0))
                .map_err(|e| format!("account lookup failed: {e}"))?
        };
        // Verified before the write, not after: an account silently pointed at
        // a logged-out directory looks healthy and fails on its first turn.
        // Asked of *this* account's CLI — asking Claude about a Codex directory
        // is how a completed login got reported as failed.
        let status = login_manager::auth_status_for(&provider, &config_dir_path)?;
        if !status.logged_in {
            let binary = crate::providers::spec(&provider).map(|s| s.binary).unwrap_or("claude");
            return Err(format!("{config_dir_path} exists but isn't logged in — log in with `{binary}` against it first"));
        }

        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE accounts SET config_dir_path = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, config_dir_path, Utc::now().to_rfc3339()],
        )
        .map_err(|e| e.to_string())?;
        log::info!("account {id} repointed at {config_dir_path}");
        conn.query_row("SELECT * FROM accounts WHERE id = ?1", params![id], row_to_account).map_err(|e| e.to_string())
    })
    .await?
}

/// One account whose provider login has expired, for the re-login banner.
///
/// Carries `checked_at` because the banner is claiming something about the
/// world at a moment in time, and a claim with no timestamp can't be judged —
/// especially on a laptop that was asleep for two days.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountNeedsLogin {
    pub account_id: String,
    pub name: String,
    pub provider: String,
    pub checked_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountAuth {
    pub account_id: String,
    pub logged_in: bool,
    pub auth_method: String,
    /// Whether Hangar can actually scope a process to this account. False for
    /// providers with no `env_for_account` equivalent (codex, gemini), which
    /// can be created but can't yet be handed to a terminal or a turn.
    pub supported: bool,
    /// The exact command a terminal for this account would run, e.g.
    /// `CLAUDE_CONFIG_DIR=/Users/me/.claude-work claude`. Built by the same
    /// `terminals::account_command_line` that `open_terminal` executes, so the
    /// string shown on the row can't drift from the one that actually runs.
    /// `None` when there's nothing runnable (CLI missing, provider not wired).
    pub command: Option<String>,
}

/// Login state for every account at once.
///
/// `check_account_auth` answers for one account, but it is a **non-async**
/// command that shells out to `claude auth status` with a 25-second
/// timeout — so it runs inline on the IPC dispatch thread and freezes the
/// whole window while it waits (the exact hazard `crate::blocking` documents).
/// Asking it once per account, as the accounts screen does, serializes that
/// cost. This runs off the dispatch thread and checks every account
/// concurrently, so a picker listing five accounts costs one CLI round-trip
/// rather than five.
#[tauri::command]
pub async fn list_account_auth(app: AppHandle) -> Result<Vec<AccountAuth>, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let rows: Vec<(String, String, String, String)> = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn
                .prepare("SELECT id, provider, billing, config_dir_path FROM accounts ORDER BY created_at ASC")
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
                .map_err(|e| e.to_string())?;
            rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?
        };

        // One thread per account that needs a real CLI check — these are
        // independent subprocess calls, so the wall-clock cost is one check
        // rather than the sum of them.
        let handles: Vec<_> = rows
            .into_iter()
            .map(|(id, provider, billing, config_dir_path)| {
                std::thread::spawn(move || {
                    // Supported means "there's a way to ask this CLI whether
                    // it's logged in" — which is now true of Codex too, and was
                    // hardcoded to Claude.
                    let supported = crate::providers::can_scope_to_account(&provider);
                    let probe_provider = provider.clone();
                    let account = crate::terminals::AccountForTerminal {
                        provider,
                        billing: billing.clone(),
                        config_dir_path: config_dir_path.clone(),
                        name: String::new(),
                    };
                    // Same short-circuits as `check_account_auth`: an API key
                    // is its own proof of auth, and an unsupported provider
                    // has nothing to ask.
                    let (logged_in, auth_method) = if billing == "key" {
                        (true, "api_key".to_string())
                    } else if !supported {
                        (false, "unsupported".to_string())
                    } else {
                        match login_manager::auth_status_for(&probe_provider, &config_dir_path) {
                            Ok(status) => (status.logged_in, status.auth_method),
                            Err(e) => {
                                log::warn!("list_account_auth: check failed for account {id}: {e}");
                                (false, "unknown".to_string())
                            }
                        }
                    };
                    // An account that isn't logged in gets its login command,
                    // since that's what selecting it would actually run.
                    let init = if logged_in { crate::terminals::InitCommand::Start } else { crate::terminals::InitCommand::Login };
                    // No session id in the *displayed* example command — each
                    // real terminal gets its own, and `--session-id <uuid>`
                    // would just be noise on the account row.
                    let command = crate::terminals::account_command_line(&account, init, None, None).ok();
                    AccountAuth { account_id: id, logged_in, auth_method, supported, command }
                })
            })
            .collect();

        Ok(handles
            .into_iter()
            .filter_map(|h| h.join().ok())
            .collect())
    })
    .await?
}

/// Which accounts can be asked whether they are still logged in.
///
/// The three exclusions are the whole point of doing this here rather than in a
/// query: a key-billed account carries its own proof, a provider with no status
/// probe cannot be asked at all (and must not be reported either way), and an
/// account whose config dir is gone would have the CLI answer about somebody
/// else's credentials.
fn probeable_accounts(conn: &rusqlite::Connection) -> Result<Vec<(String, String, String, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, provider, config_dir_path FROM accounts WHERE billing = 'sub' ORDER BY created_at ASC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;

    Ok(rows
        .into_iter()
        .filter(|(_, _, provider, config_dir)| {
            let probeable = crate::providers::spec(provider).is_some_and(|s| !matches!(s.auth_status, crate::providers::AuthStatusProbe::None));
            probeable && std::path::Path::new(config_dir).is_dir()
        })
        .collect())
}

/// Re-checks every account that can be asked, and records the answers.
///
/// Providers expire a login every so often. Before this, Hangar found out the
/// way the user did — by opening a terminal and watching the agent refuse to
/// start — so this runs on its own slow cycle and feeds the re-login banner.
///
/// `TrustMarkerFile::No` asks the CLI instead of trusting the credential file's
/// existence — which is what makes an expired Claude login detectable at all.
/// It does *not* rescue Codex: its own `login status` reports any `auth.json`
/// that exists as a live session, so a stale Codex account still reads as
/// healthy here. See `TrustMarkerFile` for the measurements behind both claims.
pub fn refresh_account_logins<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(db) = app.try_state::<DbConnection>() else { return false };
    let accounts = match db.0.lock().map_err(|e| e.to_string()).and_then(|conn| probeable_accounts(&conn)) {
        Ok(accounts) => accounts,
        Err(e) => {
            log::warn!("login check: could not read accounts: {e}");
            return false;
        }
    };
    if accounts.is_empty() {
        return false;
    }

    // One thread per account, like `list_account_auth`: these are independent
    // subprocess calls, so the wall-clock cost is one probe rather than their
    // sum.
    let handles: Vec<_> = accounts
        .into_iter()
        .map(|(id, _name, provider, config_dir)| {
            std::thread::spawn(move || {
                let result = login_manager::auth_status_with(&provider, &config_dir, login_manager::TrustMarkerFile::No);
                (id, result)
            })
        })
        .collect();

    let mut changed = false;
    for handle in handles {
        let Ok((id, result)) = handle.join() else { continue };
        let (logged_in, error, email) = match result {
            Ok(status) => (status.logged_in, None, status.email),
            // A probe that couldn't run says nothing about the credential. It's
            // recorded as an error and *not* as a logout, so a machine with no
            // network doesn't get told to sign in again.
            Err(e) => {
                log::warn!("login check: probe failed for account {id}: {e}");
                (true, Some(e), None)
            }
        };
        // The subscription's email, which this probe has always returned and
        // this loop used to drop on the floor. It is the only identifier that
        // means the same thing on another machine — `accounts.id` is minted per
        // install and `name` is whatever was typed there — so it is what lets a
        // per-profile report add up two Macs. Only ever written, never cleared:
        // a probe that failed knows nothing, and forgetting on a bad day would
        // split one subscription into two.
        if let Some(email) = email.filter(|e| !e.trim().is_empty()) {
            if let Ok(conn) = db.0.lock() {
                if let Err(e) = conn.execute("UPDATE accounts SET provider_email = ?2 WHERE id = ?1", params![id, email.trim()]) {
                    log::warn!("login check: could not record the provider email for account {id}: {e}");
                }
            }
        }
        if let Ok(conn) = db.0.lock() {
            let previous: Option<bool> = conn
                .query_row("SELECT logged_in FROM account_login_checks WHERE account_id = ?1", params![id], |row| row.get::<_, i64>(0))
                .ok()
                .map(|v| v != 0);
            if previous != Some(logged_in) {
                changed = true;
            }
            if let Err(e) = conn.execute(
                "INSERT INTO account_login_checks (account_id, checked_at, logged_in, error) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(account_id) DO UPDATE SET checked_at = excluded.checked_at, logged_in = excluded.logged_in, error = excluded.error",
                params![id, Utc::now().to_rfc3339(), logged_in as i64, error],
            ) {
                log::warn!("login check: could not record account {id}: {e}");
            }
        }
    }
    changed
}

/// The accounts whose provider login has expired — what the banner renders.
///
/// Read from the table rather than probed here: the screen asking must not wait
/// on a CLI round trip per account, and the background cycle has already paid
/// for the answer.
#[tauri::command]
pub fn list_accounts_needing_login(db: State<DbConnection>) -> Result<Vec<AccountNeedsLogin>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT a.id, a.name, a.provider, c.checked_at
             FROM account_login_checks c JOIN accounts a ON a.id = c.account_id
             WHERE c.logged_in = 0
             ORDER BY a.created_at ASC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok(AccountNeedsLogin { account_id: row.get(0)?, name: row.get(1)?, provider: row.get(2)?, checked_at: row.get(3)? })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Re-checks one account now — for the moment a login terminal exits, so the
/// banner clears without waiting for the next cycle.
#[tauri::command]
pub async fn recheck_account_login(app: AppHandle, id: String) -> Result<bool, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (provider, config_dir): (String, String) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT provider, config_dir_path FROM accounts WHERE id = ?1", params![id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| format!("account lookup failed: {e}"))?
        };
        let status = login_manager::auth_status_with(&provider, &config_dir, login_manager::TrustMarkerFile::No)?;
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO account_login_checks (account_id, checked_at, logged_in, error) VALUES (?1, ?2, ?3, NULL)
             ON CONFLICT(account_id) DO UPDATE SET checked_at = excluded.checked_at, logged_in = excluded.logged_in, error = NULL",
            params![id, Utc::now().to_rfc3339(), status.logged_in as i64],
        )
        .map_err(|e| e.to_string())?;
        Ok(status.logged_in)
    })
    .await?
}

#[tauri::command]
pub async fn delete_account(app: AppHandle, id: String) -> Result<(), String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let config_dir_path: Option<String> = conn
            .query_row("SELECT config_dir_path FROM accounts WHERE id = ?1", params![id], |row| row.get(0))
            .ok();

        // A subsession can belong to a different account than its parent, so
        // deleting an account can take worktrees with it too — clean those up
        // before the cascade removes the session rows that reference them.
        {
            let mut stmt = conn.prepare(
                "SELECT s.worktree_path, s.worktree_branch, s.worktree_kind, p.path
                 FROM sessions s
                 JOIN projects p ON p.id = s.project_id
                 WHERE s.account_id = ?1 AND s.worktree_path IS NOT NULL",
            );
            if let Ok(ref mut stmt) = stmt {
                let rows = stmt.query_map(params![id], |row| {
                    Ok(WorktreeCleanupRow {
                        worktree_path: row.get(0)?,
                        worktree_branch: row.get(1)?,
                        worktree_kind: row.get(2)?,
                        project_path: row.get(3)?,
                    })
                });
                if let Ok(rows) = rows {
                    let rows: Vec<_> = rows.filter_map(Result::ok).collect();
                    cleanup_worktree_rows(&rows);
                }
            }
        }
        drop(conn);

        if let Some(dir) = &config_dir_path {
            // Best-effort real sign-out before wiping the dir — ignore failures
            // (e.g. already logged out, or dir already gone) since we delete the
            // directory unconditionally right after.
            let mut cmd = Command::new(crate::providers::claude::binary());
            cmd.args(crate::providers::claude::logout_args());
            cmd.env("CLAUDE_CONFIG_DIR", dir);
            let _ = crate::process_timeout::run_with_timeout(cmd, crate::process_timeout::DEFAULT_TIMEOUT);
            let _ = std::fs::remove_dir_all(dir);
        }

        credential_vault::delete(&id)?;
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM accounts WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::sync::Mutex as StdMutex;

    fn app_with_account(config_dir: &str) -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Personal','personal','',?1,'acc1','healthy','now','now')",
            params![config_dir],
        )
        .unwrap();
        app.manage(DbConnection(StdMutex::new(conn)));
        app
    }

    /// Who gets asked "are you still logged in", and who must not be.
    ///
    /// Each exclusion is a way the banner could lie. An API key is its own
    /// proof, so probing one would report a logout that never happens. A
    /// provider with no status probe cannot be asked at all — reporting it
    /// either way would be invention. And a config dir that's gone would have
    /// the CLI answer about whatever credentials it finds instead, which is
    /// somebody else's account.
    #[test]
    fn only_accounts_that_can_honestly_be_asked_are_probed() {
        let dir = std::env::temp_dir().join(format!("hangar-test-login-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let present = dir.to_string_lossy().to_string();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();

        let mut add = |id: &str, provider: &str, billing: &str, config_dir: &str| {
            conn.execute(
                "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
                 VALUES (?1,?2,?3,?1,'personal','',?4,?1,'healthy','now','now')",
                params![id, provider, billing, config_dir],
            )
            .unwrap();
        };
        add("claude-sub", "claude", "sub", &present);
        add("codex-sub", "codex", "sub", &present);
        add("claude-key", "claude", "key", &present);
        add("gemini-sub", "gemini", "sub", &present);
        add("claude-gone", "claude", "sub", "/nonexistent/hangar-test");

        let probeable = probeable_accounts(&conn).unwrap();
        let ids: Vec<&str> = probeable.iter().map(|(id, _, _, _)| id.as_str()).collect();

        assert!(ids.contains(&"claude-sub"), "a subscription account with a real config dir is the whole point");
        assert!(ids.contains(&"codex-sub"), "codex has a status probe too");
        assert!(!ids.contains(&"claude-key"), "an API key is its own proof of auth");
        assert!(!ids.contains(&"gemini-sub"), "gemini has no status probe — it can't be reported either way");
        assert!(!ids.contains(&"claude-gone"), "a missing config dir would make the CLI answer about someone else");

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn stored_config_dir(app: &tauri::App<tauri::test::MockRuntime>) -> String {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().unwrap();
        conn.query_row("SELECT config_dir_path FROM accounts WHERE id = 'acc1'", [], |r| r.get(0)).unwrap()
    }

    /// Repointing has to fail *before* the write. An account silently aimed at
    /// a directory that isn't there looks perfectly healthy in the UI and only
    /// falls over on its next turn, long after the mistake was made.
    #[test]
    fn repointing_at_a_missing_directory_is_refused_and_changes_nothing() {
        let app = app_with_account("/original/config");
        let missing = std::env::temp_dir().join(format!("hangar-does-not-exist-{}", Uuid::new_v4()));

        let result = tauri::async_runtime::block_on(set_account_config_dir(
            app.handle().clone(),
            "acc1".to_string(),
            missing.to_string_lossy().to_string(),
        ));

        assert!(result.is_err(), "a non-existent directory must be rejected");
        assert_eq!(stored_config_dir(&app), "/original/config", "the row must be untouched after a rejected repoint");
    }

    /// The more dangerous case: a directory that exists but was never logged
    /// into looks plausible, so presence on disk isn't enough — it has to be
    /// confirmed with the CLI.
    #[test]
    fn repointing_at_a_logged_out_directory_is_refused_and_changes_nothing() {
        let empty = std::env::temp_dir().join(format!("hangar-empty-config-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&empty).unwrap();
        let app = app_with_account("/original/config");

        let result = tauri::async_runtime::block_on(set_account_config_dir(
            app.handle().clone(),
            "acc1".to_string(),
            empty.to_string_lossy().to_string(),
        ));

        assert!(result.is_err(), "an existing but logged-out directory must be rejected");
        assert_eq!(stored_config_dir(&app), "/original/config");

        let _ = std::fs::remove_dir_all(&empty);
    }
}
