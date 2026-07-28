//! Which git identity and credential a project's repo needs.
//!
//! The problem this exists for: the default identity on a machine is one
//! person, and a repo belonging to another account needs another. Until now
//! Hangar committed with whatever `git config --global user.name` happened to
//! say — silently, and into permanent history.
//!
//! Three things get conflated in git and are kept apart here:
//!
//! - **Identity** (`user.name` / `user.email`) decides who authored a commit.
//!   Not secret, so it goes into the worktree's own config where the agent's
//!   commits pick it up too, not just Hangar's.
//! - **The push credential** authenticates to the remote. Secret, so it lives in
//!   the OS keychain and is handed to git through `GIT_ASKPASS` at the moment of
//!   the call.
//! - **The `gh` token**, which is what every PR feature in the app runs on. A
//!   repo belonging to another GitHub account otherwise shows someone else's
//!   pull requests, or none.
//!
//! **The token is never written into `.git/config` or the remote URL.** Putting
//! it in the URL (`https://user:token@github.com/…`) is the easy way and it
//! leaves the secret inside the repo, in `git remote -v`, and in every error
//! message that quotes the URL back at you.

use crate::db::DbConnection;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};

/// What the form shows and stores. Deliberately without the token: this crosses
/// to the frontend, and a secret that only ever travels one way can't be
/// accidentally rendered, logged, or put in a screenshot.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIdentity {
    pub project_id: String,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub auth_user: Option<String>,
    /// Whether a token is stored, not what it is.
    pub has_token: bool,
}

fn credential_ref(project_id: &str) -> String {
    format!("git:{project_id}")
}

#[tauri::command]
pub fn get_project_git_identity(db: tauri::State<DbConnection>, project_id: String) -> Result<Option<GitIdentity>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    load_identity(&conn, &project_id)
}

pub(crate) fn load_identity(conn: &Connection, project_id: &str) -> Result<Option<GitIdentity>, String> {
    conn.query_row(
        "SELECT user_name, user_email, auth_user, credential_ref FROM project_git_identity WHERE project_id = ?1",
        params![project_id],
        |row| {
            let credential: Option<String> = row.get(3)?;
            Ok(GitIdentity {
                project_id: project_id.to_string(),
                user_name: row.get(0)?,
                user_email: row.get(1)?,
                auth_user: row.get(2)?,
                has_token: credential.is_some(),
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
}

/// Saves the identity, and the token if one was typed.
///
/// `token: None` means "leave whatever is stored alone" — the form never
/// receives the token back, so it can't send it again, and treating an empty
/// field as "delete it" would wipe the credential every time someone edits the
/// email.
#[tauri::command]
pub fn set_project_git_identity(
    db: tauri::State<DbConnection>,
    project_id: String,
    user_name: Option<String>,
    user_email: Option<String>,
    auth_user: Option<String>,
    token: Option<String>,
) -> Result<GitIdentity, String> {
    let blank_to_none = |v: Option<String>| v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let user_name = blank_to_none(user_name);
    let user_email = blank_to_none(user_email);
    let auth_user = blank_to_none(auth_user);
    let token = blank_to_none(token);

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let existing_ref: Option<String> = conn
        .query_row("SELECT credential_ref FROM project_git_identity WHERE project_id = ?1", params![project_id], |r| r.get(0))
        .optional()
        .map_err(|e| e.to_string())?
        .flatten();

    let credential_ref = match token {
        Some(token) => {
            let key = credential_ref(&project_id);
            crate::credential_vault::store(&key, &token)?;
            Some(key)
        }
        None => existing_ref,
    };

    conn.execute(
        "INSERT INTO project_git_identity (project_id, user_name, user_email, auth_user, credential_ref, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(project_id) DO UPDATE SET user_name = excluded.user_name, user_email = excluded.user_email,
           auth_user = excluded.auth_user, credential_ref = excluded.credential_ref, updated_at = excluded.updated_at",
        params![project_id, user_name, user_email, auth_user, credential_ref, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;

    load_identity(&conn, &project_id)?.ok_or_else(|| "identity vanished immediately after being written".to_string())
}

/// Forgets the identity **and the token**. Both, because leaving a keychain
/// entry behind for a project that no longer references it is a secret nobody
/// can find to delete later.
#[tauri::command]
pub fn clear_project_git_identity(db: tauri::State<DbConnection>, project_id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM project_git_identity WHERE project_id = ?1", params![project_id])
        .map_err(|e| e.to_string())?;
    drop(conn);
    crate::credential_vault::delete(&credential_ref(&project_id))
}

/// The askpass helper git calls when it needs a username or a password.
///
/// git runs `$GIT_ASKPASS <prompt>` and reads one line of stdout. The prompt
/// distinguishes the two questions ("Username for 'https://github.com'" vs
/// "Password for …"), which is why this can't simply echo the token: answering
/// the username question with the token would authenticate as a user called
/// `ghp_…`.
///
/// Written to disk once and reused. It reads the values out of the environment
/// rather than embedding them, so the file itself holds no secret even if it
/// outlives the process that wrote it.
const ASKPASS_SCRIPT: &str = r#"#!/bin/sh
case "$1" in
  *[Uu]sername*) printf '%s\n' "$HANGAR_GIT_USER" ;;
  *) printf '%s\n' "$HANGAR_GIT_TOKEN" ;;
esac
"#;

fn askpass_path<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join("git-askpass.sh");
    // Rewritten every time rather than only when missing: it's three lines, and
    // an old copy from a previous version answering the wrong question would be
    // invisible until a push failed.
    std::fs::write(&path, ASKPASS_SCRIPT).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

/// Environment that makes git, and `gh`, act as this project's identity.
///
/// Returned as pairs so it can go into a `Command` for Hangar's own calls **and**
/// into the terminal's PTY — which matters more than it sounds: the process that
/// runs `git push` most often in Hangar is the agent, not Hangar.
pub fn git_env_for_project<R: tauri::Runtime>(app: &tauri::AppHandle<R>, project_id: &str) -> Vec<(String, String)> {
    use tauri::Manager;
    let Some(db) = app.try_state::<DbConnection>() else { return Vec::new() };
    let identity = {
        let Ok(conn) = db.0.lock() else { return Vec::new() };
        match load_identity(&conn, project_id) {
            Ok(Some(identity)) => identity,
            _ => return Vec::new(),
        }
    };

    let mut env = Vec::new();
    // Author *and* committer: git will otherwise take the committer from the
    // global config, so a commit ends up half attributed to each person.
    if let Some(name) = &identity.user_name {
        env.push(("GIT_AUTHOR_NAME".to_string(), name.clone()));
        env.push(("GIT_COMMITTER_NAME".to_string(), name.clone()));
    }
    if let Some(email) = &identity.user_email {
        env.push(("GIT_AUTHOR_EMAIL".to_string(), email.clone()));
        env.push(("GIT_COMMITTER_EMAIL".to_string(), email.clone()));
    }

    if identity.has_token {
        if let Ok(token) = crate::credential_vault::get(&credential_ref(project_id)) {
            if let Ok(path) = askpass_path(app) {
                env.push(("GIT_ASKPASS".to_string(), path.to_string_lossy().to_string()));
                env.push(("HANGAR_GIT_USER".to_string(), identity.auth_user.clone().unwrap_or_default()));
                env.push(("HANGAR_GIT_TOKEN".to_string(), token.clone()));
                // Without this a missing or wrong credential makes git sit on a
                // hidden prompt forever instead of failing.
                env.push(("GIT_TERMINAL_PROMPT".to_string(), "0".to_string()));
            }
            // Every PR feature in the app runs through `gh`, which otherwise
            // uses whichever single account is logged in.
            env.push(("GH_TOKEN".to_string(), token));
        }
    }
    env
}

/// Writes the identity into a worktree's own git config.
///
/// Environment variables cover Hangar's own commits, but the agent runs `git
/// commit` itself in that directory, in a process Hangar doesn't control the
/// environment of once the shell is live. Local config is what both of them
/// read — and it's not a secret, so it can safely live in the repo.
pub fn apply_identity_to_worktree<R: tauri::Runtime>(app: &tauri::AppHandle<R>, project_id: &str, worktree: &std::path::Path) {
    use tauri::Manager;
    let Some(db) = app.try_state::<DbConnection>() else { return };
    let identity = {
        let Ok(conn) = db.0.lock() else { return };
        match load_identity(&conn, project_id) {
            Ok(Some(identity)) => identity,
            _ => return,
        }
    };

    for (key, value) in [("user.name", identity.user_name), ("user.email", identity.user_email)] {
        let Some(value) = value else { continue };
        if let Err(e) = crate::commands::pull_requests::git_capture(worktree, &["config", key, &value]) {
            log::warn!("git identity: couldn't set {key} in {}: {e}", worktree.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('p1','P','/tmp','now')", []).unwrap();
        conn
    }

    /// The identity crosses to the frontend; the token must not. `has_token` is
    /// the only thing the form needs to know, and a secret that never travels
    /// outward can't be rendered, logged or screenshotted by accident.
    #[test]
    fn the_identity_that_reaches_the_frontend_carries_no_token() {
        let conn = seed();
        conn.execute(
            "INSERT INTO project_git_identity (project_id, user_name, user_email, auth_user, credential_ref, updated_at)
             VALUES ('p1','Work Me','work@example.com','work-login','git:p1','now')",
            [],
        )
        .unwrap();

        let identity = load_identity(&conn, "p1").unwrap().unwrap();
        assert_eq!(identity.user_name.as_deref(), Some("Work Me"));
        assert_eq!(identity.auth_user.as_deref(), Some("work-login"));
        assert!(identity.has_token, "the form needs to know one exists");

        // Serialized shape: no field can carry the secret out.
        let json = serde_json::to_string(&identity).unwrap();
        assert!(!json.contains("git:p1"), "not even the keychain key: {json}");
        assert!(json.contains("hasToken"));
    }

    #[test]
    fn a_project_with_no_identity_reads_as_none_rather_than_an_empty_one() {
        let conn = seed();
        assert_eq!(load_identity(&conn, "p1").unwrap(), None);
    }

    /// The askpass helper has to tell the two questions apart. Answering the
    /// username prompt with the token would authenticate as a user literally
    /// named `ghp_…`, and the failure looks like a wrong password.
    #[test]
    fn the_askpass_helper_answers_username_and_password_differently() {
        let dir = std::env::temp_dir().join(format!("hangar-askpass-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("askpass.sh");
        std::fs::write(&script, ASKPASS_SCRIPT).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        }

        let run = |prompt: &str| {
            let out = std::process::Command::new(&script)
                .arg(prompt)
                .env("HANGAR_GIT_USER", "work-login")
                .env("HANGAR_GIT_TOKEN", "ghp_secret")
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };

        assert_eq!(run("Username for 'https://github.com': "), "work-login");
        assert_eq!(run("Password for 'https://work-login@github.com': "), "ghp_secret");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
