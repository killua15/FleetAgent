//! Best-effort real macOS notifications for the events Settings → General
//! lets the user opt into. Every call here must be silently swallow-able —
//! a missed notification (permission not granted, plugin error, DB
//! momentarily locked) is never worth breaking a turn or a permission
//! response over, so nothing in this module returns a `Result` the caller
//! has to handle.

use crate::commands::settings::read_app_settings;
use crate::db::DbConnection;
use crate::models::AppSettings;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;

/// What a notification is about — the groups the in-app panel shows. Only
/// what's worth a row: PRs, tasks, approvals, and the half-hourly summary of
/// agents waiting. Quota, spend cap and updates have their own banners and go
/// to the OS only.
pub(crate) mod kind {
    pub const APPROVAL: &str = "approval";
    pub const AGENT: &str = "agent";
    pub const PR: &str = "pr";
    pub const TASK: &str = "task";
}

/// Where clicking a notification takes you.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    /// `terminal` | `session` | `pr` | `account` | `update` | `task`.
    pub kind: String,
    pub id: Option<String>,
}

impl Target {
    pub fn new(kind: &str, id: impl Into<String>) -> Option<Target> {
        Some(Target { kind: kind.to_string(), id: Some(id.into()) })
    }

    /// A screen rather than one thing on it — "go to the terminals".
    pub fn screen(kind: &str) -> Option<Target> {
        Some(Target { kind: kind.to_string(), id: None })
    }
}

/// One row of the in-app notification list.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppNotification {
    pub id: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub target: Option<Target>,
    pub created_at: String,
    pub read_at: Option<String>,
    pub ai_category: Option<String>,
    pub ai_priority: Option<i64>,
    pub ai_reason: Option<String>,
}

/// The list keeps the newest this many; older ones fall off.
const KEEP: i64 = 500;

/// A second notification about the same thing within this window refreshes the
/// first instead of adding a row — an agent finishing five turns in a minute is
/// one thing to look at, not five.
const DEDUPE_SECS: i64 = 120;

/// Stores a notification, merging it into a recent one about the same target,
/// and trims the list. Separate from `record` so it can be tested against a
/// plain connection.
pub(crate) fn persist(conn: &Connection, kind: &str, title: &str, body: &str, target: Option<&Target>, now: chrono::DateTime<chrono::Utc>) -> Result<String, String> {
    let target_json = target.map(|t| serde_json::to_string(t).unwrap_or_default());
    let since = (now - chrono::Duration::seconds(DEDUPE_SECS)).to_rfc3339();
    let existing: Option<String> = match &target_json {
        Some(json) => conn
            .query_row(
                "SELECT id FROM notifications WHERE kind = ?1 AND target_json = ?2 AND created_at >= ?3 ORDER BY created_at DESC LIMIT 1",
                params![kind, json, since],
                |r| r.get(0),
            )
            .ok(),
        None => None,
    };
    let id = match existing {
        Some(id) => {
            conn.execute(
                "UPDATE notifications SET title = ?2, body = ?3, created_at = ?4, read_at = NULL, ai_category = NULL, ai_priority = NULL, ai_reason = NULL WHERE id = ?1",
                params![id, title, body, now.to_rfc3339()],
            )
            .map_err(|e| e.to_string())?;
            id
        }
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            conn.execute(
                "INSERT INTO notifications (id, kind, title, body, target_json, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, kind, title, body, target_json, now.to_rfc3339()],
            )
            .map_err(|e| e.to_string())?;
            id
        }
    };
    conn.execute(
        "DELETE FROM notifications WHERE id NOT IN (SELECT id FROM notifications ORDER BY created_at DESC LIMIT ?1)",
        params![KEEP],
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

pub(crate) fn unread_count(conn: &Connection) -> i64 {
    conn.query_row("SELECT COUNT(*) FROM notifications WHERE read_at IS NULL", [], |r| r.get(0)).unwrap_or(0)
}

/// The unread count on the Dock icon. A badge stays visible with Do Not
/// Disturb on — the whole reason this list exists.
pub(crate) fn refresh_badge<R: Runtime>(app: &AppHandle<R>, unread: i64) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_badge_count(if unread > 0 { Some(unread) } else { None });
    }
    let _ = app.emit("notifications-changed", unread);
}

/// Records a notification in Hangar's own list — always — and also shows it
/// as an OS notification when the user turned that kind on.
///
/// Always, because the OS is exactly the part that can't be relied on: with Do
/// Not Disturb on, an OS notification arrives nowhere, and before this list
/// existed that meant it never happened as far as the user could tell.
pub(crate) fn record<R: Runtime>(
    app: &AppHandle<R>,
    kind: &str,
    is_enabled: impl FnOnce(&AppSettings) -> bool,
    title: &str,
    body: &str,
    target: Option<Target>,
) {
    if let Some(db) = app.try_state::<DbConnection>() {
        let stored = db.0.lock().map_err(|e| e.to_string()).and_then(|conn| {
            let id = persist(&conn, kind, title, body, target.as_ref(), chrono::Utc::now())?;
            Ok((id, unread_count(&conn)))
        });
        match stored {
            Ok((id, unread)) => {
                let _ = app.emit("notification-added", id);
                refresh_badge(app, unread);
            }
            Err(e) => log::warn!("notification not recorded ({title}): {e}"),
        }
    }
    notify_if_enabled(app, is_enabled, title, body);
}

fn row_to_notification(row: &rusqlite::Row) -> rusqlite::Result<AppNotification> {
    let target_json: Option<String> = row.get("target_json")?;
    Ok(AppNotification {
        id: row.get("id")?,
        kind: row.get("kind")?,
        title: row.get("title")?,
        body: row.get("body")?,
        target: target_json.and_then(|j| serde_json::from_str(&j).ok()),
        created_at: row.get("created_at")?,
        read_at: row.get("read_at")?,
        ai_category: row.get("ai_category")?,
        ai_priority: row.get("ai_priority")?,
        ai_reason: row.get("ai_reason")?,
    })
}

/// Unread notifications the AI hasn't sorted yet, newest first.
pub(crate) fn unread_unclassified(conn: &Connection) -> Result<Vec<AppNotification>, String> {
    let mut stmt = conn
        .prepare("SELECT * FROM notifications WHERE read_at IS NULL AND ai_category IS NULL ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_to_notification).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// The list, newest first.
#[tauri::command]
pub fn list_notifications<R: Runtime>(app: AppHandle<R>) -> Result<Vec<AppNotification>, String> {
    let db = app.state::<DbConnection>();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT * FROM notifications ORDER BY created_at DESC").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_to_notification).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn unread_notification_count<R: Runtime>(app: AppHandle<R>) -> Result<i64, String> {
    let db = app.state::<DbConnection>();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(unread_count(&conn))
}

/// Marks the given notifications read, or every one when `ids` is `None`.
#[tauri::command]
pub fn mark_notifications_read<R: Runtime>(app: AppHandle<R>, ids: Option<Vec<String>>) -> Result<(), String> {
    let unread = {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        mark_read(&conn, ids.as_deref(), &chrono::Utc::now().to_rfc3339())?;
        unread_count(&conn)
    };
    refresh_badge(&app, unread);
    Ok(())
}

pub(crate) fn mark_read(conn: &Connection, ids: Option<&[String]>, now: &str) -> Result<(), String> {
    match ids {
        None => conn.execute("UPDATE notifications SET read_at = ?1 WHERE read_at IS NULL", params![now]).map(|_| ()),
        Some(ids) => ids
            .iter()
            .try_for_each(|id| conn.execute("UPDATE notifications SET read_at = ?2 WHERE id = ?1 AND read_at IS NULL", params![id, now]).map(|_| ())),
    }
    .map_err(|e| e.to_string())
}

/// Deletes the notifications of one kind, or all of them — or, with
/// `read_only`, just the ones already seen, so what's still pending stays put.
pub(crate) fn clear(conn: &Connection, kind: Option<&str>, read_only: bool) -> Result<(), String> {
    let seen = if read_only { " AND read_at IS NOT NULL" } else { "" };
    match kind {
        Some(k) => conn.execute(&format!("DELETE FROM notifications WHERE kind = ?1{seen}"), params![k]),
        None => conn.execute(&format!("DELETE FROM notifications WHERE 1 = 1{seen}"), []),
    }
    .map(|_| ())
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn clear_notifications<R: Runtime>(app: AppHandle<R>, kind: Option<String>, read_only: Option<bool>) -> Result<(), String> {
    let unread = {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        clear(&conn, kind.as_deref(), read_only.unwrap_or(false))?;
        unread_count(&conn)
    };
    refresh_badge(&app, unread);
    Ok(())
}

/// Puts the badge back on launch, so a restart doesn't hide what's unread.
pub(crate) fn restore_badge<R: Runtime>(app: &AppHandle<R>) {
    let unread = app.try_state::<DbConnection>().and_then(|db| db.0.lock().ok().map(|conn| unread_count(&conn))).unwrap_or(0);
    refresh_badge(app, unread);
}

/// Reads `app_settings`, checks `is_enabled` against it, and — only if
/// true — shows a real OS notification. Two DB round-trips (settings, plus
/// whatever the caller already did to build `body`) are cheap next to the
/// events this fires on (a turn finishing, a rate limit, a permission
/// prompt), none of which happen more than a few times a minute.
pub(crate) fn notify_if_enabled<R: Runtime>(app: &AppHandle<R>, is_enabled: impl FnOnce(&AppSettings) -> bool, title: &str, body: &str) {
    let Some(db) = app.try_state::<DbConnection>() else { return };
    let Ok(conn) = db.0.lock() else { return };
    let Ok(settings) = read_app_settings(&conn) else { return };
    drop(conn);
    if !is_enabled(&settings) {
        return;
    }
    // Still best-effort — a missed notification never breaks a turn — but no
    // longer *silent*. Swallowing the error is why an enabled setting and a
    // mute app were indistinguishable for months: the code looked right, the
    // toggles were on, and nothing ever reached the screen.
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        log::warn!("notification not delivered ({title}): {e}");
    }
}

/// Called from `src/lib/updates.ts` once per installed version — the
/// updater check itself runs in JS (`@tauri-apps/plugin-updater`), so this is
/// the bridge back into the one place that's allowed to show a real
/// notification. The frontend is responsible for not calling this twice for
/// the same version (it re-checks every 30 minutes).
#[tauri::command]
pub fn notify_update_available<R: Runtime>(app: AppHandle<R>, version: String) {
    // OS only — the update banner is the in-app signal.
    notify_if_enabled(&app, |s| s.notify_update_available, "Hangar update ready", &format!("v{version} is installed — restart Hangar to run it"));
}

/// Sends one on demand and reports what happened.
///
/// Every silent subsystem in this app needed one of these before anyone could
/// tell "working" from "quietly failing" — the updater needed it, the vault
/// needed it. This is that button for notifications: the error reaches the
/// screen instead of a log nobody opens.
#[tauri::command]
pub fn send_test_notification<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    app.notification()
        .builder()
        .title("Hangar")
        .body("Notifications are working. This is what an agent finishing looks like.")
        .show()
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::NOTIFICATIONS_DDL).unwrap();
        conn
    }

    fn count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0)).unwrap()
    }

    /// The same thing twice in quick succession is one row to look at, back to
    /// unread — not two.
    #[test]
    fn a_repeat_about_the_same_target_refreshes_the_row() {
        let conn = conn();
        let t0 = chrono::Utc::now();
        let target = Target::new("terminal", "t1");
        let first = persist(&conn, kind::AGENT, "Agent is waiting", "one", target.as_ref(), t0).unwrap();
        mark_read(&conn, None, &t0.to_rfc3339()).unwrap();
        let again = persist(&conn, kind::AGENT, "Agent is waiting", "two", target.as_ref(), t0 + chrono::Duration::seconds(30)).unwrap();
        assert_eq!(first, again);
        assert_eq!(count(&conn), 1);
        assert_eq!(unread_count(&conn), 1, "the refresh makes it unread again");
        let body: String = conn.query_row("SELECT body FROM notifications", [], |r| r.get(0)).unwrap();
        assert_eq!(body, "two");

        // After the window, or about something else, it's a new row.
        persist(&conn, kind::AGENT, "Agent is waiting", "three", target.as_ref(), t0 + chrono::Duration::seconds(DEDUPE_SECS + 60)).unwrap();
        persist(&conn, kind::AGENT, "Agent is waiting", "other", Target::new("terminal", "t2").as_ref(), t0).unwrap();
        persist(&conn, kind::PR, "New pull request", "no target", None, t0).unwrap();
        persist(&conn, kind::PR, "New pull request", "no target", None, t0).unwrap();
        assert_eq!(count(&conn), 5, "untargeted notifications are never merged");
    }

    #[test]
    fn the_list_keeps_the_newest_ones_only() {
        let conn = conn();
        let t0 = chrono::Utc::now();
        for i in 0..(KEEP + 25) {
            persist(&conn, kind::PR, "n", &i.to_string(), None, t0 + chrono::Duration::seconds(i)).unwrap();
        }
        assert_eq!(count(&conn), KEEP);
        let oldest: String = conn.query_row("SELECT body FROM notifications ORDER BY created_at ASC LIMIT 1", [], |r| r.get(0)).unwrap();
        assert_eq!(oldest, "25", "the oldest ones fell off");
    }

    #[test]
    fn marking_read_by_id_or_all() {
        let conn = conn();
        let now = chrono::Utc::now();
        let a = persist(&conn, kind::PR, "a", "", Target::new("pr", "1").as_ref(), now).unwrap();
        persist(&conn, kind::PR, "b", "", Target::new("pr", "2").as_ref(), now).unwrap();
        mark_read(&conn, Some(&[a]), &now.to_rfc3339()).unwrap();
        assert_eq!(unread_count(&conn), 1);
        mark_read(&conn, None, &now.to_rfc3339()).unwrap();
        assert_eq!(unread_count(&conn), 0);
    }

    /// "Clear seen" must never take something still waiting on you with it.
    #[test]
    fn clearing_the_seen_ones_keeps_the_pending_ones() {
        let conn = conn();
        let now = chrono::Utc::now();
        let seen_pr = persist(&conn, kind::PR, "a", "", Target::new("pr", "1").as_ref(), now).unwrap();
        let seen_task = persist(&conn, kind::TASK, "b", "", Target::new("task", "1").as_ref(), now).unwrap();
        persist(&conn, kind::PR, "c", "", Target::new("pr", "2").as_ref(), now).unwrap();
        mark_read(&conn, Some(&[seen_pr, seen_task]), &now.to_rfc3339()).unwrap();

        clear(&conn, Some(kind::PR), true).unwrap();
        assert_eq!(count(&conn), 2, "only the seen PR went: the seen task is another kind, the pending PR isn't seen");

        clear(&conn, None, true).unwrap();
        assert_eq!(count(&conn), 1);
        assert_eq!(unread_count(&conn), 1);

        clear(&conn, None, false).unwrap();
        assert_eq!(count(&conn), 0, "and plain clear still clears everything");
    }
}
