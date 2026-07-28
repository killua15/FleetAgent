use keyring::Entry;

/// Keychain service name. Renamed with the product; no migration from the
/// pre-rename `com.killua.fleet` service is needed or included — that service
/// was verified to hold no entries at rename time (both existing accounts are
/// subscription-billed, and only API-key accounts ever store a secret here).
/// An API key that predated the rename would need re-entering, nothing worse.
const SERVICE: &str = "com.killua.hangar";

/// Credentials never touch the SQLite database or the frontend — only a
/// `credential_ref` (the account id) is stored there. The actual secret
/// (API key or CLI session token) lives in the OS keychain.
fn entry(credential_ref: &str) -> Result<Entry, String> {
    Entry::new(SERVICE, credential_ref).map_err(|e| e.to_string())
}

pub fn store(credential_ref: &str, secret: &str) -> Result<(), String> {
    entry(credential_ref)?
        .set_password(secret)
        .map_err(|e| e.to_string())
}

pub fn get(credential_ref: &str) -> Result<String, String> {
    entry(credential_ref)?
        .get_password()
        .map_err(|e| e.to_string())
}

/// Whether the secret is actually readable *right now*.
///
/// The database row and the keychain item are separate facts, and they come
/// apart: a keychain entry written by one build of an unsigned app is not
/// always readable by the next one, and a user can delete it from Keychain
/// Access at any time. Anything that reports "token stored" has to ask the
/// keychain rather than trust the row, or it promises something that fails
/// later somewhere with no obvious connection to this.
pub fn has(credential_ref: &str) -> bool {
    get(credential_ref).is_ok()
}

pub fn delete(credential_ref: &str) -> Result<(), String> {
    match entry(credential_ref)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
