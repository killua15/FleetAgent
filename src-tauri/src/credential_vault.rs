use keyring::Entry;

const SERVICE: &str = "com.killua.fleet";

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

pub fn delete(credential_ref: &str) -> Result<(), String> {
    match entry(credential_ref)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}
