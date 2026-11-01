mod agent_mcp;
mod ai_oneshot;
mod app_data;
mod auto_continue;
mod auth;
mod blocking;
mod capabilities;
mod bridge;
mod commands;
mod credential_vault;
mod db;
mod diagrams;
mod factory;
mod forges;
mod git_identity;
mod graph;
mod handoff;
mod knowledge_sources;
mod login_manager;
mod memory_pressure;
mod models;
mod notification_classify;
mod notifications;
mod permission_bridge;
mod process_manager;
mod process_timeout;
mod providers;
mod quota;
mod rotation;
mod run_queue;
mod task_sources;
mod terminals;
mod tray;
mod turn_runner;
mod updater;
mod usage_meter;
mod usage_scraper;

use db::DbConnection;
use login_manager::LoginRegistry;
use permission_bridge::PendingApprovals;
use process_manager::ProcessRegistry;
use std::sync::Mutex;
use tauri::{Emitter, Manager};
use terminals::{RestoreGuard, TerminalRegistry};
use turn_runner::{CancelledTurns, SpendCapNotified, TurnRegistry};
use usage_scraper::UsageCache;

/// Kills everything Hangar spawned before the process goes away. None of these
/// children die with their parent on their own — a headless turn, a PTY
/// login, and a PTY session all outlive it happily — so without this a quit
/// leaves `claude` processes running against the user's repos, writing to
/// transcripts nobody is reading anymore.
fn shutdown_children(app: &tauri::AppHandle) {
    turn_runner::kill_all_turns(app);
    // Nothing the Factory started outlives Hangar either.
    factory::procs::kill_everything();
    process_manager::kill_all(app);
    login_manager::kill_all_logins(app);
    terminals::kill_all(app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
/// Un bucle de fondo que sobrevive a que su cuerpo entre en panic.
///
/// `thread::spawn(move || loop { … })` a secas tiene un final malo: si el cuerpo
/// entra en panic una sola vez, el hilo muere y esa función no vuelve **hasta
/// que se reinicia la app**. Los seis bucles de abajo son el latido de cosas
/// que el usuario da por hechas —la salida de las terminales, el estado de las
/// sesiones, el sondeo de PRs— así que perder uno se siente exactamente como
/// que Hangar se colgó, que es el fallo que esto viene a cerrar.
///
/// Cada vuelta va envuelta, no el bucle entero: envolver el bucle solo movería
/// el problema un nivel. Un panic se registra con el nombre del latido y la
/// siguiente vuelta ocurre igual.
/// `AssertUnwindSafe` porque `AppHandle` lleva mutabilidad interior y no es
/// `RefUnwindSafe`. Es correcto aquí y no una tirita: tras un panic esto solo
/// registra y vuelve a empezar de cero, sin leer nada que el panic haya podido
/// dejar a medias. Y el estado compartido vive tras mutex, que al envenenarse
/// hacen que los ticks salgan por su rama `else` en vez de propagar — cada uno
/// de ellos ya está escrito como `let Ok(..) = ..lock() else { return }`.
fn heartbeat(name: &'static str, body: impl Fn() + Send + 'static) {
    std::thread::spawn(move || loop {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(&body)).is_err() {
            // El hook por defecto ya ha escrito el panic y su traza; esto es lo
            // que dice cuál de los seis fue y que sigue vivo.
            log::error!("background loop '{name}' panicked — it keeps running, but something in it is broken");
        }
    });
}

pub fn run() {
    let builder = tauri::Builder::default();
    // First, as the plugin asks: a second launch hands over to this one and
    // exits before anything else starts. See Cargo.toml for why not on macOS.
    #[cfg(not(target_os = "macos"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| tray::show_main(app)));
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir { file_name: Some("hangar".to_string()) }),
                ])
                .build(),
        )
        .setup(|app| {
            // Before anything touches the app data dir: bring it over from
            // the pre-rename identifier if this machine ran the old build.
            // `db::init` creates the directory, so a later call would find a
            // fresh empty one and skip the migration.
            app_data::migrate(&app.handle());
            // Off the startup path: a keychain call can prompt or stall.
            std::thread::spawn(updater::forget_legacy_token);
            let conn = match db::init(&app.handle()) {
                Ok(conn) => conn,
                Err(e) => {
                    // A dialog and a log line, then out — rather than the
                    // crash reporter, which tells the user nothing and tells
                    // us nothing either.
                    log::error!("database unavailable: {e}");
                    tauri_plugin_dialog::DialogExt::dialog(app.handle())
                        .message(format!(
                            "Hangar can't open its database.\n\n{e}\n\nThe log is in ~/Library/Logs/com.killua.hangar/hangar.log"
                        ))
                        .kind(tauri_plugin_dialog::MessageDialogKind::Error)
                        .title("Hangar")
                        .blocking_show();
                    std::process::exit(1);
                }
            };
            app.manage(DbConnection(Mutex::new(conn)));
            // What was unread when Hangar last closed is still unread now.
            notifications::restore_badge(&app.handle());
            app.manage(ProcessRegistry::new());
            app.manage(LoginRegistry::new());
            app.manage(TurnRegistry::new());
            app.manage(CancelledTurns::new());
            app.manage(SpendCapNotified::new());
            app.manage(UsageCache::new());
            app.manage(PendingApprovals::new());
            app.manage(TerminalRegistry::new());
            app.manage(RestoreGuard::new());
            app.manage(task_sources::cache::TaskCache::new());
            // After everything it reads (terminals, notifications, task cache)
            // is managed.
            tray::init(&app.handle());
            let tray_handle = app.handle().clone();
            heartbeat("tray", move || {
                std::thread::sleep(tray::TICK);
                tray::refresh_title(&tray_handle);
            });
            // Agents waiting on you, summed up every half hour — instead of a
            // notification per finished turn, which with twenty panels was a
            // stream nobody could read.
            let digest_handle = app.handle().clone();
            heartbeat("agent-digest", move || {
                std::thread::sleep(terminals::AGENT_DIGEST_EVERY);
                terminals::agent_digest(&digest_handle);
            });

            // Before the poller thread below: the user's declarations about which
            // host is which forge have to be in place for the *first* pass, or it
            // runs on the heuristics they already overrode. A failure here is not
            // fatal — the app falls back to guessing, which is what it did before
            // declarations existed.
            if let Err(e) = commands::forge_status::load_declared_hosts(&app.state::<DbConnection>()) {
                log::error!("could not load declared forge instances, falling back to host heuristics: {e}");
            }
            // So "what Hangar needs" knows a wiki is connected without a DB read.
            if let Ok(conn) = app.state::<DbConnection>().0.lock() {
                knowledge_sources::refresh_connected(&conn);
            }

            // Background PR-comment poller — Hangar has no public endpoint
            // for a real GitHub webhook, so this is the same shape a
            // desktop app like Conductor uses: poll while the app is open,
            // plus the "check now" command for an immediate refresh.
            //
            // One thread per forge instance, fanned out fresh on every cycle.
            // Per instance because that is the unit that can be slow or logged
            // out, and a `glab` that hangs on a self-hosted GitLab used to
            // delay every GitHub project queued behind it. Re-read each cycle
            // rather than spawned once, so an instance added after launch is
            // picked up and one whose last project went away stops being
            // polled — a fixed set of threads would do neither.
            //
            // The PR screen reads the database these fill, so what they really
            // buy is a screen that paints with no network in its way.
            // The Factory picks up where it was: a line that was running
            // before Hangar closed goes on, a limit wait included.
            factory::env::warm_up();
            factory::looping::resume_all(app.handle());

            let poll_handle = app.handle().clone();
            heartbeat("pr-poll", move || {
                std::thread::sleep(std::time::Duration::from_secs(300));
                let hosts = commands::pull_requests::hosts_with_projects(&poll_handle);
                let mut passes = Vec::new();
                for host in hosts {
                    let handle = poll_handle.clone();
                    passes.push(std::thread::spawn(move || {
                        commands::pull_requests::poll_open_prs_for_host(&handle, Some(&host));
                    }));
                }
                // Joined before sleeping again: overlapping cycles would have
                // two passes writing the same instance's rows at once.
                for pass in passes {
                    let _ = pass.join();
                }
            });

            // Background task-board read — what keeps the Tasks panel next to
            // the terminals current, and what notices a task landing on you or
            // changing column while you're heads-down. Every five minutes like
            // the PR poll: about a dozen calls an hour per board, far inside
            // GitHub's and ClickUp's limits. The first read is the baseline and
            // announces nothing, so it runs soon after launch — the panel would
            // otherwise sit empty for five minutes.
            let task_handle = app.handle().clone();
            let first_task_pass = std::sync::atomic::AtomicBool::new(true);
            heartbeat("task-poll", move || {
                let wait = if first_task_pass.swap(false, std::sync::atomic::Ordering::Relaxed) { 15 } else { 300 };
                std::thread::sleep(std::time::Duration::from_secs(wait));
                if let Err(e) = task_sources::cache::refresh(&task_handle, None) {
                    log::warn!("task poll failed: {e}");
                }
            });

            // Background login check — providers expire an account's login
            // every so often, and until this existed Hangar found out the way
            // the user did: by opening a terminal and watching the agent refuse
            // to start.
            //
            // Every account that can be asked, not just the ones in use: a
            // login that lapsed on an idle account matters *before* you reach
            // for it, which is the whole point of announcing it. Half an hour
            // because logins last days — polling harder would spawn a CLI per
            // account for an answer that changes about once a month.
            //
            // First pass after a short delay rather than immediately: launch is
            // already busy, and this is the least urgent thing competing for it.
            let login_handle = app.handle().clone();
            // La espera corta de la primera vuelta se conserva con una bandera:
            // `heartbeat` no distingue la primera de las demás, y arrancar el
            // sondeo de logins en el instante del lanzamiento es justo lo que
            // este bucle evitaba.
            let first_login_pass = std::sync::atomic::AtomicBool::new(true);
            heartbeat("account-logins", move || {
                let wait = if first_login_pass.swap(false, std::sync::atomic::Ordering::Relaxed) { 20 } else { 1800 };
                std::thread::sleep(std::time::Duration::from_secs(wait));
                if commands::accounts::refresh_account_logins(&login_handle) {
                    // The banner and the accounts screen both read the
                    // table this just wrote.
                    let _ = login_handle.emit("account-logins-checked", ());
                }
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
            // One-second tick that turns "no output for a while" into a state
            // change. Cheap by construction — it walks the open terminals and
            // touches nothing else — and it's what lets the app tell you an
            // agent finished instead of making you notice.
            let state_handle = app.handle().clone();
            heartbeat("terminal-states", move || {
                std::thread::sleep(std::time::Duration::from_secs(1));
                terminals::tick_terminal_states(&state_handle);
                terminals::tick_auto_continue(&state_handle);
            });

            // Own, faster loop — the mobile tail's `EMIT_DEBOUNCE` (250ms) is
            // well under the 1s cadence above, and piggybacking on that tick
            // used to add up to another second of latency on top of it. This
            // one only does work when a PTY terminal without a session
            // actually has redacted text piled up.
            let mobile_tail_handle = app.handle().clone();
            heartbeat("mobile-tail", move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                terminals::tick_mobile_tail(&mobile_tail_handle);
            });

            // Hands PTY output to the panes displaying it, in one message per
            // terminal per frame — the reader threads only queue it (see
            // `terminals::TerminalSink`). A steady 60Hz rather than backing off
            // when idle: the first bytes after a quiet spell are usually the
            // echo of a keystroke, and anything slower than a frame reads as a
            // laggy terminal. Each tick is a mutex check and nothing else when
            // no terminal has spoken.
            let output_handle = app.handle().clone();
            heartbeat("terminal-output", move || {
                std::thread::sleep(terminals::OUTPUT_FLUSH_INTERVAL);
                terminals::tick_terminal_output(&output_handle);
            });

            let usage_handle = app.handle().clone();
            heartbeat("account-usage", move || {
                terminals::poll_account_usage(&usage_handle);
                // Right after sampling, because that's the only moment the
                // answer to "does any account have room" can have changed.
                run_queue::tick_queue(&usage_handle);
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
            commands::accounts::list_accounts_needing_login,
            commands::accounts::recheck_account_login,
            commands::accounts::list_discoverable_logins,
            commands::accounts::set_account_config_dir,
            commands::accounts::delete_account,
            commands::accounts::list_provider_models,
            commands::projects::list_projects,
            commands::projects::create_project,
            commands::projects::ensure_project_for_path,
            commands::projects::update_project,
            commands::projects::set_project_hidden,
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
            commands::sessions::remove_worktree,
            commands::sessions::prune_worktrees,
            commands::sessions::adopt_worktree,
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
            commands::preferences::get_global_agent_preferences,
            commands::preferences::set_global_agent_preferences,
            commands::preferences::get_project_agent_preferences,
            commands::preferences::set_project_agent_preferences,
            commands::skills::list_skills,
            commands::skills::save_skill,
            commands::skills::delete_skill,
            commands::skills::reveal_skill_directory,
            commands::skills::list_project_skills,
            commands::skills::set_project_skill,
            commands::skills::list_always_on_skills,
            commands::skills::set_always_on_skill,
            commands::diagrams::list_project_diagrams,
            commands::diagrams::open_diagram,
            commands::diagrams::generate_project_diagram,
            commands::skill_sources::suggested_skill_sources,
            commands::skill_sources::list_skill_sources,
            commands::skill_sources::add_skill_source,
            commands::skill_sources::remove_skill_source,
            commands::skill_sources::list_source_skills,
            commands::skill_sources::preview_source_skill,
            commands::skill_sources::install_source_skill,
            commands::skill_sources::check_skill_updates,
            commands::skill_sources::update_installed_skills,
            commands::settings::list_project_account_rotation,
            commands::settings::set_project_account_allowed,
            commands::forge_status::list_forge_instances,
            commands::forge_status::list_forge_hosts,
            commands::forge_status::recheck_forge_instances,
            commands::forge_status::save_forge_instance,
            commands::forge_status::delete_forge_instance,
            commands::forge_status::authenticate_forge_instance,
            commands::forge_status::list_instance_repos,
            commands::forge_status::clone_instance_repo,
            commands::requirements::list_requirements,
            commands::requirements::recheck_requirements,
            commands::requirements::check_capability,
            commands::graph::graphify_status,
            commands::graph::get_project_graph,
            commands::graph::build_project_graph,
            commands::graph::open_graph_html,
            commands::graph::graph_storage_dir,
            commands::graph::list_graph_groups,
            commands::graph::create_graph_group,
            commands::graph::delete_graph_group,
            commands::graph::set_graph_group_member,
            commands::graph::get_graph_group_members,
            commands::graph::get_graph_group_state,
            commands::graph::get_graph_bridges,
            commands::graph::build_graph_group,
            commands::graph::graph_savings_report,
            commands::graph::graph_collisions,
            commands::graph::set_graph_index_policy,
            commands::graph::inspect_project_corpus,
            commands::onboarding::onboarding_progress,
            rotation::get_rotation_policy,
            rotation::set_rotation_policy,
            rotation::get_project_fallback_account,
            rotation::set_project_fallback_account,
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
            commands::pull_requests::check_open_prs_now,
            commands::pull_requests::list_pr_host_checks,
            commands::git_panel::get_git_overview,
            commands::git_panel::list_task_worktrees,
            commands::git_panel::git_fetch,
            commands::git_panel::git_pull,
            commands::git_panel::git_push,
            commands::git_panel::open_path_in_terminal,
            commands::git_panel::is_rider_installed,
            commands::git_panel::open_in_rider,
            commands::git_panel::list_branches,
            commands::git_panel::list_commits,
            commands::git_panel::get_commit_files,
            commands::git_panel::get_commit_file_diff,
            commands::git_panel::checkout_branch,
            commands::git_panel::get_worktree_changes,
            commands::git_panel::get_worktree_file_diff,
            commands::agent_nodes::list_agent_nodes,
            commands::agent_nodes::create_agent_node,
            commands::agent_nodes::update_agent_node,
            commands::agent_nodes::delete_agent_node,
            commands::agent_nodes::run_agent_tree,
            terminals::open_terminal,
            terminals::open_login_terminal,
            terminals::attach_terminal,
            terminals::detach_terminal,
            terminals::write_terminal,
            terminals::get_terminal_tail,
            terminals::resize_terminal,
            terminals::close_terminal,
            terminals::list_terminals,
            terminals::terminal_branches,
            terminals::restore_terminals,
            terminals::resolve_terminal_target,
            terminals::reopen_terminal_with_account,
            rotation::suggest_terminal_account,
            rotation::list_terminal_rotation_accounts,
            commands::pull_requests::open_pr_review_terminal,
            commands::sessions::create_worktree_for_project,
            commands::sessions::list_files,
            commands::sessions::read_file,
            terminals::fan_out_prompt,
            terminals::open_task_worktrees,
            commands::sessions::check_branch_availability,
            git_identity::get_project_git_identity,
            git_identity::set_project_git_identity,
            git_identity::clear_project_git_identity,
            task_sources::list_task_sources,
            task_sources::save_task_source,
            task_sources::delete_task_source,
            task_sources::link_task_source,
            task_sources::unlink_task_source,
            task_sources::list_task_source_links,
            task_sources::list_source_scopes,
            task_sources::list_project_tasks,
            task_sources::list_source_tasks,
            task_sources::cache::list_cached_tasks,
            task_sources::cache::refresh_tasks,
            factory::factory_list_projects,
            factory::factory_get_project,
            factory::factory_create_project,
            factory::remove::factory_remove_project,
            factory::factory_update_project,
            factory::factory_repo_status,
            factory::factory_init_repo,
            factory::brain::factory_brain_parse,
            factory::brain::factory_brain_guide,
            factory::brain::factory_brain_example,
            factory::brain::factory_read_markdown,
            factory::brain::factory_brain_seal,
            factory::brain::factory_brain_lint,
            factory::brain::factory_brain_from_rules,
            factory::brain::factory_brain_get,
            factory::brain::factory_brain_versions,
            factory::brain::factory_brain_export,
            factory::brain::factory_brain_session,
            factory::brain::factory_brain_generate,
            factory::brain::factory_brain_read_generated,
            factory::brain::factory_amendments_list,
            factory::brain::factory_amendment_set_status,
            factory::team::factory_team_get,
            factory::team::factory_team_save,
            factory::team::factory_models,
            factory::team::factory_team_suggestions,
            factory::team::factory_team_apply_suggestions,
            factory::planner::factory_plan_trd,
            factory::planner::factory_plan_approve,
            factory::planner::factory_plan_cancel,
            factory::planner::factory_orders_export,
            factory::planner::factory_orders_sync,
            factory::planner::factory_orders_list,
            factory::planner::factory_orders_reorder,
            factory::planner::factory_order_add,
            factory::planner::factory_trd_refine,
            factory::planner::factory_trd_read,
            factory::view::factory_live,
            factory::view::factory_deliveries,
            factory::looping::factory_loop_status,
            factory::looping::factory_play,
            factory::looping::factory_pause,
            factory::looping::factory_stop,
            factory::looping::factory_stop_all,
            factory::looping::factory_run_resolve,
            factory::looping::factory_run_check,
            factory::looping::factory_run_console,
            factory::looping::factory_start_anyway,
            factory::looping::factory_order_retry,
            factory::prepare::factory_prepare,
            knowledge_sources::list_knowledge_sources,
            knowledge_sources::save_knowledge_source,
            knowledge_sources::delete_knowledge_source,
            terminals::list_terminal_states,
            terminals::list_auto_continue,
            terminals::set_terminal_auto_continue,
            terminals::rename_terminal,
            terminals::set_terminal_minimized,
            run_queue::enqueue_run,
            run_queue::list_queued_runs,
            run_queue::cancel_queued_run,
            quota::list_account_quotas,
            usage_meter::list_account_spend,
            usage_meter::list_project_spend,
            usage_meter::usage_report,
            usage_meter::daily_usage_since,
            updater::log_update_error,
            terminals::refresh_all_quotas,
            notifications::send_test_notification,
            notifications::notify_update_available,
            notifications::list_notifications,
            notifications::unread_notification_count,
            notifications::mark_notifications_read,
            notifications::clear_notifications,
            notification_classify::classify_notifications,
            bridge::session::bridge_status,
            bridge::session::set_machine_name,
            bridge::session::get_default_terminal_mode,
            bridge::session::set_default_terminal_mode,
            bridge::pairing::bridge_begin_pairing,
            bridge::pairing::bridge_cancel_pairing,
            bridge::pairing::bridge_accept_pairing,
            bridge::session::bridge_revoke_device,
            bridge::dispatch::bridge_receive,
            bridge::dispatch::bridge_reply,
            bridge::dispatch::bridge_event,
            bridge::selection::bridge_select,
            bridge::selection::bridge_selection,
            quota::list_project_budgets,
            quota::get_project_budget,
            quota::set_project_budget,
            quota::project_fan_out,
            auth::start_oauth_callback,
            auth::store_refresh_token,
            auth::load_refresh_token,
            auth::clear_refresh_token,
            auth::record_auth_check,
            auth::last_auth_check,
            auth::auth_grace_is_live,
        ])
        // With the menu-bar icon, closing the window hides Hangar instead of
        // quitting: the agents and the polls keep running, and the icon (or the
        // Dock) brings it back. Quit is in the icon's menu and ⌘Q.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" && tray::hides_on_close(window.app_handle()) {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
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
            // Clicking the Dock icon while the window is hidden in the menu bar.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { has_visible_windows: false, .. } = event {
                tray::show_main(app_handle);
            }
        });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// Sin esto, un panic en el cuerpo mataba el hilo y esa función no volvía
    /// hasta reiniciar la app — el síntoma que el usuario describe como «se
    /// queda colgado y tengo que cerrar y abrir».
    #[test]
    fn a_panicking_heartbeat_keeps_beating() {
        let beats = Arc::new(AtomicUsize::new(0));
        let seen = beats.clone();
        super::heartbeat("test", move || {
            let n = beats.fetch_add(1, Ordering::SeqCst);
            // Solo la primera vuelta explota: si el bucle muriera con ella, el
            // contador se quedaría en 1 para siempre.
            if n == 0 {
                panic!("primera vuelta rota a propósito");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        });

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while seen.load(Ordering::SeqCst) < 4 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            seen.load(Ordering::SeqCst) >= 4,
            "el latido murió con el panic: solo {} vueltas",
            seen.load(Ordering::SeqCst)
        );
    }
}
