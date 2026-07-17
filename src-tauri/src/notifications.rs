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
    let _ = app.notification().builder().title(title).body(body).show();
}
