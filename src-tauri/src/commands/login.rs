use crate::login_manager::{self, AuthStatus, LoginSession};
use tauri::AppHandle;

#[tauri::command]
pub fn begin_login(app: AppHandle, provider: String, name: String) -> Result<LoginSession, String> {
    login_manager::begin_login(app, provider, name)
}

#[tauri::command]
pub fn send_login_input(app: AppHandle, login_id: String, data: String) -> Result<(), String> {
    login_manager::send_login_input(&app, &login_id, data)
}

#[tauri::command]
pub fn check_login_status(app: AppHandle, login_id: String) -> Result<AuthStatus, String> {
    login_manager::check_login_status(&app, &login_id)
}

#[tauri::command]
pub fn cancel_login(app: AppHandle, login_id: String) -> Result<(), String> {
    login_manager::cancel_login(&app, &login_id)
}
