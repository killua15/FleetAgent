/// Runs a blocking closure (subprocess calls, arbitrary shell commands, curl)
/// off Tauri's IPC-dispatch thread. A `#[tauri::command] fn` without `async`
/// runs inline on whatever thread dispatches the IPC message — on a desktop
/// webview that's effectively the main UI thread, so any such command that
/// blocks (a `gh`/`git` subprocess, a `curl` call, an arbitrary
/// `setup_command`) freezes the whole app for as long as it takes, not just
/// the button that triggered it. `spawn_blocking` moves the work onto its own
/// dedicated thread pool for blocking I/O — separate from the async
/// executor's worker threads — so this holds up fine even with several calls
/// in flight (e.g. checking multiple PRs in sequence).
pub(crate) async fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| format!("background task failed: {e}"))
}
