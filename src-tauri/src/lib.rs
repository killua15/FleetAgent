mod commands;
mod credential_vault;
mod db;
mod login_manager;
mod models;
mod process_manager;
mod providers;
mod turn_runner;

use db::DbConnection;
use login_manager::LoginRegistry;
use process_manager::ProcessRegistry;
use std::sync::Mutex;
use tauri::Manager;
use turn_runner::TurnRegistry;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let conn = db::init(&app.handle());
            app.manage(DbConnection(Mutex::new(conn)));
            app.manage(ProcessRegistry::new());
            app.manage(LoginRegistry::new());
            app.manage(TurnRegistry::new());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::accounts::list_accounts,
            commands::accounts::list_available_providers,
            commands::accounts::validate_api_key,
            commands::accounts::create_account,
            commands::accounts::check_account_auth,
            commands::accounts::delete_account,
            commands::accounts::list_provider_models,
            commands::projects::list_projects,
            commands::projects::create_project,
            commands::sessions::start_session,
            commands::sessions::list_project_sessions,
            commands::sessions::list_workspace_sessions,
            commands::sessions::get_session_view,
            commands::sessions::update_session_settings,
            commands::sessions::send_message,
            commands::sessions::stop_session,
            commands::sessions::delete_session,
            commands::sessions::list_session_blocks,
            commands::sessions::list_resumable_conversations,
            commands::sessions::set_session_resume_target,
            commands::attachments::save_attachment,
            commands::login::begin_login,
            commands::login::send_login_input,
            commands::login::check_login_status,
            commands::login::cancel_login,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
