//! The credential that lets this machine read updates from a private repo.
//!
//! Hangar's releases live in a repository that isn't public, and GitHub serves
//! nothing from one without authentication. The updater therefore needs a
//! token — and the obvious place to put it, baked into the binary, is the wrong
//! one: every copy of the app would carry a key to the repo, and handing
//! someone the app would hand them read access without either of you noticing.
//!
//! So it lives in the OS keychain, set once per machine. The binary ships with
//! nothing, and an app without the token simply never finds an update instead
//! of failing.
//!
//! This is also why the endpoint is the *Contents* API rather than a release
//! asset: a private release's asset id changes with every release, and the
//! updater needs a URL that doesn't move.

const CREDENTIAL_REF: &str = "update-access-token";

/// The headers the updater needs, or none when no token is stored.
///
/// `vnd.github.raw` is what turns the Contents API from "JSON describing the
/// file" into the file itself — without it the updater would be handed base64
/// wrapped in metadata and would rightly refuse to parse it.
#[tauri::command]
pub fn update_request_headers() -> Option<std::collections::BTreeMap<String, String>> {
    headers_for(CREDENTIAL_REF)
}

/// Parameterised by credential so the tests can each use their own entry —
/// they share one keychain and run in parallel, and a shared key made them
/// race each other rather than test anything.
fn headers_for(credential_ref: &str) -> Option<std::collections::BTreeMap<String, String>> {
    let token = crate::credential_vault::get(credential_ref).ok()?;
    if token.trim().is_empty() {
        return None;
    }
    Some(
        [
            ("Authorization".to_string(), format!("Bearer {}", token.trim())),
            ("Accept".to_string(), "application/vnd.github.raw".to_string()),
            ("X-GitHub-Api-Version".to_string(), "2022-11-28".to_string()),
        ]
        .into_iter()
        .collect(),
    )
}

/// Whether this machine has a token stored — asked of the keychain, not of a
/// flag, for the same reason the task-source form asks: the two come apart.
#[tauri::command]
pub fn has_update_access() -> bool {
    crate::credential_vault::has(CREDENTIAL_REF)
}

/// Stores the token, or clears it when given nothing.
#[tauri::command]
pub fn set_update_access(token: Option<String>) -> Result<(), String> {
    store_for(CREDENTIAL_REF, token)
}

fn store_for(credential_ref: &str, token: Option<String>) -> Result<(), String> {
    match token.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) {
        Some(token) => crate::credential_vault::store(credential_ref, &token),
        None => crate::credential_vault::delete(credential_ref),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No token is a normal state — a fresh machine, or someone who never set
    /// one up — and it has to mean "no updates available", not a broken app.
    #[test]
    fn no_token_means_no_headers_rather_than_empty_ones() {
        let key = format!("hangar-test-update-none-{}", uuid::Uuid::new_v4());
        assert_eq!(headers_for(&key), None);
        assert!(!crate::credential_vault::has(&key));
    }

    /// The raw media type is the whole reason this endpoint works: without it
    /// GitHub answers with base64 inside JSON and the manifest never parses.
    #[test]
    fn a_stored_token_produces_headers_that_ask_for_the_raw_file() {
        let key = format!("hangar-test-update-{}", uuid::Uuid::new_v4());
        store_for(&key, Some("ghp_example".to_string())).unwrap();

        let headers = headers_for(&key).expect("a stored token must produce headers");
        assert_eq!(headers.get("Authorization").map(String::as_str), Some("Bearer ghp_example"));
        assert_eq!(headers.get("Accept").map(String::as_str), Some("application/vnd.github.raw"));

        store_for(&key, None).unwrap();
        assert!(!crate::credential_vault::has(&key), "clearing has to actually remove it, not blank it");
    }
}
