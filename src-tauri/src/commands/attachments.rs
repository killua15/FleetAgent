use tauri::{AppHandle, Manager};

/// Only alphanumerics, `.`, `-`, `_` survive — anything else (path separators
/// included) becomes `_`, so a crafted file name can't escape the
/// attachment's own directory.
fn sanitize_file_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Persists a pasted/dropped image to disk and returns its absolute path, so
/// it can be referenced by path in a turn's prompt text — `claude -p` has no
/// `--image` flag, but Claude Code's own Read tool loads image files given a
/// path mentioned in the conversation.
#[tauri::command]
pub fn save_attachment(app: AppHandle, session_id: String, file_name: String, data: Vec<u8>) -> Result<String, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| e.to_string())?
        .join("attachments")
        .join(&session_id);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let unique_name = format!("{}-{}", uuid::Uuid::new_v4(), sanitize_file_name(&file_name));
    let path = dir.join(unique_name);
    std::fs::write(&path, data).map_err(|e| e.to_string())?;

    Ok(path.to_string_lossy().to_string())
}
