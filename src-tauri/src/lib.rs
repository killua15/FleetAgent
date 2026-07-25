mod blocking;
mod commands;
mod credential_vault;
mod db;
mod handoff;
mod login_manager;
mod models;
mod notifications;
mod permission_bridge;
mod process_manager;
mod process_timeout;
mod providers;
mod rotation;
mod terminals;
mod turn_runner;
mod usage_scraper;

use db::DbConnection;
use login_manager::LoginRegistry;
use permission_bridge::PendingApprovals;
use process_manager::ProcessRegistry;
use std::sync::Mutex;
use tauri::Manager;
use terminals::{RestoreGuard, TerminalRegistry};
use turn_runner::{CancelledTurns, SpendCapNotified, TurnRegistry};
use usage_scraper::UsageCache;

/// Kills everything Fleet spawned before the process goes away. None of these
/// children die with their parent on their own — a headless turn, a PTY
/// login, and a PTY session all outlive it happily — so without this a quit
/// leaves `claude` processes running against the user's repos, writing to
/// transcripts nobody is reading anymore.
fn shutdown_children(app: &tauri::AppHandle) {
    turn_runner::kill_all_turns(app);
    process_manager::kill_all(app);
    login_manager::kill_all_logins(app);
    terminals::kill_all(app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // File + stdout logging. `eprintln!` was invisible in a bundled
        // `.app` (stderr goes nowhere), which made the app's signature
        // failure mode — "I sent a message and nothing happened" —
        // undiagnosable without attaching a debugger. The LogDir target
        // writes somewhere a user can actually be asked to send from.
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir { file_name: Some("fleet".to_string()) }),
                ])
                .build(),
        )
        .setup(|app| {
            let conn = db::init(&app.handle());
            app.manage(DbConnection(Mutex::new(conn)));
            app.manage(ProcessRegistry::new());
            app.manage(LoginRegistry::new());
            app.manage(TurnRegistry::new());
            app.manage(CancelledTurns::new());
            app.manage(SpendCapNotified::new());
            app.manage(UsageCache::new());
            app.manage(PendingApprovals::new());
            app.manage(TerminalRegistry::new());
            app.manage(RestoreGuard::new());

            // Background PR-comment poller — Fleet has no public endpoint
            // for a real GitHub webhook, so this is the same shape a
            // desktop app like Conductor uses: poll while the app is open,
            // plus the "check now" command for an immediate refresh.
            let poll_handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(300));
                commands::pull_requests::poll_all_open_prs(&poll_handle);
            });

            // Background account-usage poller — checks `/usage` for the
            // accounts that currently have a terminal open, flags the ones out
            // of quota, and clears the flag when they recover. That's what
            // drives the "reopen this terminal in another account" banner. Kept
            // to accounts in use because each check spawns a `claude` in a PTY
            // (~6s); polling idle accounts would be pure waste.
            //
            // First pass immediately, then every 5 minutes. Sleeping first left
            // a benching written in an earlier run standing for the first five
            // minutes of every launch, which is long enough for the user to open
            // a terminal and be told an account is out of quota when it isn't.
            // The startup pass is also cheap in the common case: with no
            // terminals open yet it does the cooldown recovery and no scraping.
            let usage_handle = app.handle().clone();
            std::thread::spawn(move || loop {
                terminals::poll_account_usage(&usage_handle);
                std::thread::sleep(std::time::Duration::from_secs(300));
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::accounts::list_accounts,
            commands::accounts::list_available_providers,
            commands::accounts::validate_api_key,
            commands::accounts::create_account,
            commands::accounts::check_account_auth,
            commands::accounts::list_account_auth,
            commands::accounts::list_discoverable_logins,
            commands::accounts::set_account_config_dir,
            commands::accounts::delete_account,
            commands::accounts::list_provider_models,
            commands::projects::list_projects,
            commands::projects::create_project,
            commands::projects::ensure_project_for_path,
            commands::projects::update_project,
            commands::projects::delete_project,
            commands::sessions::start_session,
            commands::sessions::create_subsession,
            commands::sessions::list_project_sessions,
            commands::sessions::list_workspace_sessions,
            commands::sessions::get_session_view,
            commands::sessions::update_session_settings,
            commands::sessions::send_message,
            commands::sessions::stop_session,
            commands::sessions::delete_session,
            commands::sessions::list_session_blocks,
            commands::sessions::list_file_edits,
            commands::sessions::list_session_files,
            commands::sessions::read_session_file,
            commands::sessions::list_turn_usage,
            commands::sessions::get_account_usage_summary,
            usage_scraper::fetch_real_usage,
            permission_bridge::respond_to_permission_request,
            commands::sessions::list_resumable_conversations,
            commands::sessions::set_session_resume_target,
            commands::attachments::save_attachment,
            commands::login::begin_login,
            commands::login::send_login_input,
            commands::login::check_login_status,
            commands::login::cancel_login,
            commands::settings::set_account_monthly_cap,
            commands::settings::get_app_settings,
            commands::settings::set_app_settings,
            commands::settings::list_project_account_rotation,
            commands::settings::set_project_account_allowed,
            rotation::get_rotation_policy,
            rotation::set_rotation_policy,
            commands::settings::list_memory_groups,
            commands::settings::create_memory_group,
            commands::settings::delete_memory_group,
            commands::settings::list_group_membership,
            commands::settings::set_group_membership,
            commands::settings::list_group_memory_files,
            commands::settings::get_memory_file,
            commands::settings::update_memory_file_content,
            commands::settings::create_memory_file,
            commands::settings::delete_memory_file,
            commands::settings::list_discoverable_memory_files,
            commands::settings::import_discovered_memory_file,
            commands::settings::trigger_review,
            commands::pull_requests::create_pull_request,
            commands::pull_requests::preview_pull_request,
            commands::pull_requests::get_pull_request,
            commands::pull_requests::check_pr_comments,
            commands::pull_requests::send_pr_comments_to_session,
            commands::pull_requests::send_pr_comments_to_terminal,
            commands::pull_requests::dismiss_pr_comments,
            commands::pull_requests::list_pr_review_inbox,
            commands::pull_requests::check_all_open_prs_now,
            commands::git_panel::get_git_overview,
            commands::git_panel::git_fetch,
            commands::git_panel::git_pull,
            commands::git_panel::git_push,
            commands::git_panel::open_path_in_terminal,
            commands::git_panel::list_branches,
            commands::git_panel::list_commits,
            commands::git_panel::get_commit_files,
            commands::git_panel::get_commit_file_diff,
            commands::git_panel::checkout_branch,
            commands::agent_nodes::list_agent_nodes,
            commands::agent_nodes::create_agent_node,
            commands::agent_nodes::update_agent_node,
            commands::agent_nodes::delete_agent_node,
            commands::agent_nodes::run_agent_tree,
            terminals::open_terminal,
            terminals::attach_terminal,
            terminals::detach_terminal,
            terminals::write_terminal,
            terminals::resize_terminal,
            terminals::close_terminal,
            terminals::list_terminals,
            terminals::restore_terminals,
            terminals::resolve_terminal_target,
            terminals::reopen_terminal_with_account,
            rotation::suggest_terminal_account,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // `ExitRequested` fires while the app is still alive (all windows
            // closed, before teardown) and `Exit` right before the process
            // ends — reaping on both keeps the cleanup honest whether the
            // user closes the window or quits outright. `shutdown_children`
            // drains its registries, so the second call is a no-op.
            if matches!(event, tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit) {
                shutdown_children(app_handle);
            }
        });
}
