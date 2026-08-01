//! Best-effort real macOS notifications for the events Settings → General
//! lets the user opt into. Every call here must be silently swallow-able —
//! a missed notification (permission not granted, plugin error, DB
//! momentarily locked) is never worth breaking a turn or a permission
//! response over, so nothing in this module returns a `Result` the caller
//! has to handle.

use crate::commands::settings::read_app_settings;
use crate::db::DbConnection;
use crate::models::AppSettings;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_notification::NotificationExt;

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

/// Called from `UpdateBanner.tsx` once per newly-detected version — the
/// updater check itself runs in JS (`@tauri-apps/plugin-updater`), so this is
/// the bridge back into the one place that's allowed to show a real
/// notification. The frontend is responsible for not calling this twice for
/// the same version (it re-checks every 30 minutes).
#[tauri::command]
pub fn notify_update_available<R: Runtime>(app: AppHandle<R>, version: String) {
    notify_if_enabled(&app, |s| s.notify_update_available, "Hangar update available", &format!("v{version} is ready to install"));
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
