//! Hangar in the macOS menu bar / the Windows and Linux tray.
//!
//! What it's for: knowing whether anything needs you without bringing the
//! window back — which panels are working or waiting, what's on your boards,
//! and how many notifications you haven't read. And somewhere to live when the
//! window is closed, so the agents and the background polls keep going.
//!
//! The menu is a snapshot, rebuilt by a slow tick only when what it shows has
//! changed. Cheaper and simpler than wiring every event that could move a
//! number, and a menu that's a few seconds behind is still right by the time
//! you've moved the mouse to it.

use crate::db::DbConnection;
use crate::notifications;
use crate::task_sources::cache::TaskCache;
use crate::terminals::{self, TerminalState};
use std::sync::Mutex;
use tauri::menu::{Menu, MenuBuilder, MenuItemBuilder, SubmenuBuilder};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Runtime};

const TRAY_ID: &str = "hangar";
/// More panels than this in a menu is a list nobody reads; the window is the
/// place for them.
const MAX_TERMINALS: usize = 12;

/// Everything the menu shows. Compared between ticks, so a menu that hasn't
/// changed isn't rebuilt under the user's cursor.
#[derive(Debug, Default, PartialEq, Clone)]
pub(crate) struct Snapshot {
    pub terminals: Vec<(String, String, TerminalState)>,
    pub unread: i64,
    /// Per board: label, total, and `(status, count)` in board order.
    pub boards: Vec<(String, usize, Vec<(String, usize)>)>,
}

#[derive(Default)]
pub struct TrayState(Mutex<Option<Snapshot>>);

/// The menu-bar title next to the icon (macOS only): unread notifications and
/// panels waiting on you. Empty when there's nothing, so the icon sits quiet.
pub(crate) fn title(snapshot: &Snapshot) -> String {
    let waiting = snapshot.terminals.iter().filter(|t| t.2 == TerminalState::Waiting).count();
    let mut parts = Vec::new();
    if snapshot.unread > 0 {
        parts.push(format!("●{}", snapshot.unread));
    }
    if waiting > 0 {
        parts.push(format!("◐{waiting}"));
    }
    parts.join(" ")
}

fn state_glyph(state: TerminalState) -> &'static str {
    match state {
        TerminalState::Working => "●",
        TerminalState::Waiting => "◐",
        TerminalState::Idle => "○",
        TerminalState::Exited => "✕",
    }
}

/// Statuses with counts, in board order: to start, in flight, finished.
pub(crate) fn status_counts(tasks: &[crate::task_sources::Task]) -> Vec<(String, usize)> {
    let rank = |kind: Option<&str>| match kind {
        Some("open") => 0,
        Some("done") => 2,
        Some("closed") => 3,
        _ => 1,
    };
    let mut counts: Vec<(String, usize, u8)> = Vec::new();
    for task in tasks {
        let Some(status) = task.status.as_deref() else { continue };
        match counts.iter_mut().find(|c| c.0 == status) {
            Some(entry) => entry.1 += 1,
            None => counts.push((status.to_string(), 1, rank(task.status_kind.as_deref()))),
        }
    }
    counts.sort_by(|a, b| a.2.cmp(&b.2).then(a.0.cmp(&b.0)));
    counts.into_iter().map(|(s, n, _)| (s, n)).collect()
}

fn snapshot<R: Runtime>(app: &AppHandle<R>) -> Snapshot {
    let unread = app
        .try_state::<DbConnection>()
        .and_then(|db| db.0.lock().ok().map(|conn| notifications::unread_count(&conn)))
        .unwrap_or(0);
    let boards = app
        .try_state::<TaskCache>()
        .map(|cache| {
            cache
                .snapshot()
                .into_iter()
                .filter(|s| s.fetched_at.is_some())
                .map(|s| (s.label, s.tasks.len(), status_counts(&s.tasks)))
                .collect()
        })
        .unwrap_or_default();
    Snapshot { terminals: terminals::tray_rows(app), unread, boards }
}

fn build_menu<R: Runtime>(app: &AppHandle<R>, snap: &Snapshot) -> tauri::Result<Menu<R>> {
    let mut menu = MenuBuilder::new(app).text("open", "Open Hangar").separator();

    // Panels.
    let header = if snap.terminals.is_empty() { "No panels open".to_string() } else { format!("Panels ({})", snap.terminals.len()) };
    menu = menu.item(&MenuItemBuilder::with_id("panels-header", header).enabled(false).build(app)?);
    for (id, name, state) in snap.terminals.iter().take(MAX_TERMINALS) {
        menu = menu.text(format!("term:{id}"), format!("{}  {name}", state_glyph(*state)));
    }
    if snap.terminals.len() > MAX_TERMINALS {
        menu = menu.text("terminals", format!("… {} more", snap.terminals.len() - MAX_TERMINALS));
    }
    menu = menu.separator();

    // Tasks: one submenu per board, statuses inside.
    for (index, (label, total, statuses)) in snap.boards.iter().enumerate() {
        let mut sub = SubmenuBuilder::with_id(app, format!("board:{index}"), format!("☑  {label} — {total}"));
        for (status, count) in statuses {
            sub = sub.item(&MenuItemBuilder::with_id(format!("board:{index}:{status}"), format!("{status}  {count}")).enabled(false).build(app)?);
        }
        sub = sub.separator().text("tasks", "Open tasks panel");
        menu = menu.item(&sub.build()?);
    }
    if snap.boards.is_empty() {
        menu = menu.text("tasks", "☑  Tasks");
    }

    let notifications = if snap.unread > 0 { format!("🔔  Notifications ({})", snap.unread) } else { "🔔  Notifications".to_string() };
    menu = menu.text("notifications", notifications).separator().text("quit", "Quit Hangar");
    menu.build()
}

/// Brings the window back, wherever it went: hidden to the tray, minimized, or
/// behind other apps.
pub(crate) fn show_main<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Where a menu item sends the frontend. The window does the navigating — it
/// owns the routes — so the tray only says where.
#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TrayNavigate {
    screen: &'static str,
    params: serde_json::Value,
}

fn on_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    match id {
        "quit" => app.exit(0),
        "open" => show_main(app),
        "notifications" => {
            show_main(app);
            let _ = app.emit("tray-open-notifications", ());
        }
        "tasks" => {
            show_main(app);
            let _ = app.emit("tray-navigate", TrayNavigate { screen: "terminals", params: serde_json::json!({ "showTasks": true }) });
        }
        "terminals" => {
            show_main(app);
            let _ = app.emit("tray-navigate", TrayNavigate { screen: "terminals", params: serde_json::json!({}) });
        }
        other => {
            if let Some(terminal_id) = other.strip_prefix("term:") {
                show_main(app);
                let _ = app.emit("tray-navigate", TrayNavigate { screen: "terminals", params: serde_json::json!({ "focusTerminalId": terminal_id }) });
            }
        }
    }
}

/// Puts the icon in the menu bar / tray. A failure is logged and Hangar runs
/// without it — closing the window then quits, as it did before.
pub(crate) fn init<R: Runtime>(app: &AppHandle<R>) {
    app.manage(TrayState::default());
    let built = (|| -> tauri::Result<()> {
        let snap = snapshot(app);
        let menu = build_menu(app, &snap)?;
        let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-template.png"))?;
        TrayIconBuilder::with_id(TRAY_ID)
            .icon(icon)
            // Template: macOS draws it in the menu bar's own colour, light or dark.
            .icon_as_template(true)
            .tooltip("Hangar")
            .menu(&menu)
            .show_menu_on_left_click(true)
            .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
            .build(app)?;
        #[cfg(target_os = "macos")]
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            let _ = tray.set_title(Some(title(&snap)));
        }
        if let Ok(mut last) = app.state::<TrayState>().0.lock() {
            *last = Some(snap);
        }
        Ok(())
    })();
    if let Err(e) = built {
        log::error!("could not create the menu-bar icon, Hangar runs without it: {e}");
    }
}

/// Rebuilds the menu and the title if anything they show has changed.
pub(crate) fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return };
    let snap = snapshot(app);
    {
        let state = app.state::<TrayState>();
        let Ok(mut last) = state.0.lock() else { return };
        if last.as_ref() == Some(&snap) {
            return;
        }
        *last = Some(snap.clone());
    }
    match build_menu(app, &snap) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => log::warn!("could not rebuild the menu-bar menu: {e}"),
    }
    #[cfg(target_os = "macos")]
    let _ = tray.set_title(Some(title(&snap)));
}

/// Whether closing the window should hide it rather than quit: only with the
/// setting on **and** an icon to come back through.
pub(crate) fn hides_on_close<R: Runtime>(app: &AppHandle<R>) -> bool {
    if app.tray_by_id(TRAY_ID).is_none() {
        return false;
    }
    app.try_state::<DbConnection>()
        .and_then(|db| db.0.lock().ok().and_then(|conn| crate::commands::settings::read_app_settings(&conn).ok()))
        .map(|s| s.keep_in_menu_bar)
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_title_is_empty_when_nothing_needs_you() {
        let snap = Snapshot { terminals: vec![("a".into(), "x".into(), TerminalState::Working)], unread: 0, boards: vec![] };
        assert_eq!(title(&snap), "");
    }

    #[test]
    fn the_title_counts_unread_and_waiting_panels() {
        let snap = Snapshot {
            terminals: vec![
                ("a".into(), "x".into(), TerminalState::Waiting),
                ("b".into(), "y".into(), TerminalState::Waiting),
                ("c".into(), "z".into(), TerminalState::Idle),
            ],
            unread: 3,
            boards: vec![],
        };
        assert_eq!(title(&snap), "●3 ◐2");
    }

    #[test]
    fn statuses_come_in_board_order() {
        let task = |status: &str, kind: &str| crate::task_sources::Task {
            external_id: status.into(),
            key: "k".into(),
            title: "t".into(),
            description: String::new(),
            url: String::new(),
            status: Some(status.into()),
            status_kind: Some(kind.into()),
            branch_hint: None,
            source_id: "s".into(),
            source_kind: "clickup".into(),
            container: None,
            suggested_branch: String::new(),
        };
        let tasks = vec![task("done", "done"), task("verificar", "custom"), task("to do", "open"), task("verificar", "custom")];
        assert_eq!(status_counts(&tasks), vec![("to do".into(), 1), ("verificar".into(), 2), ("done".into(), 1)]);
    }
}
