//! Listing and opening the diagrams an agent delivered for a project.

use crate::diagrams::{self, Diagram};
use tauri::AppHandle;

/// What has been delivered for this project, newest first.
///
/// Empty for the common case — most projects will never have asked for a
/// diagram — and empty is not an error worth surfacing.
#[tauri::command]
pub fn list_project_diagrams(app: AppHandle, project_id: String) -> Result<Vec<Diagram>, String> {
    let app_data = crate::app_data::root(&app)?;
    Ok(diagrams::list(&diagrams::dir(&app_data, &project_id)))
}

/// Opens one delivered page in the system browser.
///
/// The browser and not a panel, for the same reason `open_graph_html` gives
/// (`commands/graph.rs`): a page like this renders blank under the app's CSP.
/// Here there is a second reason — the delivered HTML is written by whatever an
/// agent authored, and the system browser is a much better place for that than
/// inside the app's own webview.
///
/// `file_name` is the only value in this feature that comes from the frontend
/// and ends up in a system call, so it is validated rather than trusted.
#[tauri::command]
pub fn open_diagram(app: AppHandle, project_id: String, file_name: String) -> Result<(), String> {
    if !diagrams::is_safe_file_name(&file_name) {
        return Err(format!("'{file_name}' is not a diagram in this project"));
    }
    let app_data = crate::app_data::root(&app)?;
    let path = diagrams::dir(&app_data, &project_id).join(&file_name);
    if !path.is_file() {
        return Err(format!("'{file_name}' is not there any more — the list may be stale"));
    }
    tauri_plugin_opener::OpenerExt::opener(&app)
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| format!("could not open the diagram: {e}"))
}
