//! The backend half of self-updating. The check, download and install run in
//! the frontend (`src/lib/updates.ts`, on `@tauri-apps/plugin-updater`); this is
//! what that side can't do on its own.

/// Where the old GitHub update channel kept its personal access token. That
/// channel is gone — updates come from Supabase, on the tester's own session —
/// so the token only sits in the keychain as a credential nobody uses.
const LEGACY_TOKEN_REF: &str = "update-access-token";

/// Writes an update failure to `hangar.log`.
///
/// The failure is shown in the banner too, but a banner is gone once dismissed,
/// and "the update downloaded and then nothing happened" was undiagnosable
/// without a line in the log saying what the install ran into.
#[tauri::command]
pub fn log_update_error(version: Option<String>, error: String) {
    match version {
        Some(version) => log::error!("updater: installing v{version} failed: {error}"),
        None => log::error!("updater: checking for an update failed: {error}"),
    }
}

/// Removes the legacy token, once, if this machine still has one. Best-effort:
/// failing to delete an unused credential is not worth a startup error.
pub fn forget_legacy_token() {
    if crate::credential_vault::has(LEGACY_TOKEN_REF) {
        match crate::credential_vault::delete(LEGACY_TOKEN_REF) {
            Ok(()) => log::info!("updater: removed the old GitHub update token from the keychain"),
            Err(e) => log::warn!("updater: could not remove the old GitHub update token: {e}"),
        }
    }
}
