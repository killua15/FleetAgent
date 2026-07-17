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
use tauri::{AppHandle, Manager, State};
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
}

/// Lets the wizard's step 1 flag providers whose CLI isn't installed, rather
/// than letting the user pick one and only discovering it's unusable later.
#[tauri::command]
pub fn list_available_providers() -> Vec<ProviderAvailability> {
    ["claude", "codex", "gemini"]
        .into_iter()
        .map(|p| ProviderAvailability {
            provider: p.to_string(),
            available: providers::is_provider_available(p),
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
pub fn validate_api_key(provider: String, api_key: String) -> Result<ApiKeyCheck, String> {
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
}

/// Live model catalog for a given account. Key-billed accounts get the real
/// `/v1/models` list from Anthropic (genuinely "fetched via the SDK/API", not
/// hardcoded); subscription accounts have no raw API key to call that REST
/// endpoint with, so they fall back to the Claude Code CLI's own known
/// `--model` aliases — same fallback used if the live fetch fails for any
/// reason (network error, unexpected response shape, etc).
#[tauri::command]
pub fn list_provider_models(db: State<DbConnection>, account_id: String) -> Result<Vec<ModelOption>, String> {
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

    let config_dir_path = if let Some(login_id) = &account.login_id {
        // Adopt the pending login's already-authenticated config dir as-is
        // — this *is* the account's credential. Claude Code CLI's OAuth
        // token lives in the OS keychain, keyed by the literal
        // CLAUDE_CONFIG_DIR path used during `claude auth login`, not by
        // anything inside the directory's files. Moving/renaming this
        // directory to a different path orphans that keychain entry (every
        // later `claude auth status`/turn run against the new path reports
        // "not logged in" even though the login genuinely succeeded) —
        // confirmed by reproducing it with a plain `cp -R` to a new path.
        // So the account's permanent config dir must stay exactly where
        // `claude auth login` wrote it.
        let pending = login_manager::pending_dir(&app, login_id)?;
        if !pending.is_dir() {
            return Err(format!("pending login {login_id} not found or already adopted"));
        }
        login_manager::take(&app, login_id);
        pending
    } else {
        let accounts_dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("accounts");
        let config_dir_path = accounts_dir.join(&id);
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
#[tauri::command]
pub fn check_account_auth(db: State<DbConnection>, id: String) -> Result<login_manager::AuthStatus, String> {
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
        return Ok(login_manager::AuthStatus { logged_in: true, auth_method: "api_key".to_string() });
    }
    if provider != "claude" {
        return Ok(login_manager::AuthStatus { logged_in: false, auth_method: "unsupported".to_string() });
    }
    login_manager::auth_status_for_dir(&config_dir_path)
}

#[tauri::command]
pub fn delete_account(db: State<DbConnection>, id: String) -> Result<(), String> {
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
        let _ = cmd.output();
        let _ = std::fs::remove_dir_all(dir);
    }

    credential_vault::delete(&id)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM accounts WHERE id = ?1", params![id])
        .map_err(|e| e.to_string())?;
    Ok(())
}
