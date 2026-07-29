use keyring::Entry;
use std::collections::BTreeMap;
use std::sync::Mutex;

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
    forget(credential_ref);
    entry(credential_ref)?.set_password(secret).map_err(|e| e.to_string())
}

/// Secrets already read this run.
///
/// macOS asks for the login password the first time an app touches a keychain
/// item, and the app touches some of them constantly — the update token on
/// every check, a task source's on every board load. One read per launch
/// instead of one per action is the difference between a prompt you answer
/// once and a prompt that feels like the app is broken.
///
/// In memory only, and only for what was successfully read: it is the same
/// secret that is about to be handed to a subprocess anyway, and it dies with
/// the process.
static CACHE: Mutex<Option<BTreeMap<String, String>>> = Mutex::new(None);

pub fn get(credential_ref: &str) -> Result<String, String> {
    if let Ok(cache) = CACHE.lock() {
        if let Some(hit) = cache.as_ref().and_then(|map| map.get(credential_ref)) {
            return Ok(hit.clone());
        }
    }
    let secret = entry(credential_ref)?.get_password().map_err(|e| e.to_string())?;
    if let Ok(mut cache) = CACHE.lock() {
        cache.get_or_insert_with(BTreeMap::new).insert(credential_ref.to_string(), secret.clone());
    }
    Ok(secret)
}

/// Drops a cached secret, so a replaced or removed one is never served from
/// memory after the keychain has moved on.
fn forget(credential_ref: &str) {
    if let Ok(mut cache) = CACHE.lock() {
        if let Some(map) = cache.as_mut() {
            map.remove(credential_ref);
        }
    }
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
    forget(credential_ref);
    match entry(credential_ref)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

/// Los tests que tocan el llavero de verdad van de uno en uno.
///
/// `security` puede quedarse esperando una autorización del sistema, y varios a
/// la vez la convierten en una cola: la suite pasó de 14 segundos a 18 minutos y
/// uno de ellos acabó dando por perdida la lectura. No es un fallo de lógica —
/// aislado pasa — es paralelismo contra un recurso que no lo admite.
#[cfg(test)]
pub(crate) static KEYCHAIN_TEST_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    /// A replaced secret must not keep serving the old one from memory — the
    /// cache exists to stop repeated password prompts, not to outlive the
    /// keychain it mirrors.
    #[test]
    fn replacing_a_secret_invalidates_what_was_cached() {
        let _serial = KEYCHAIN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let key = format!("hangar-test-cache-{}", uuid::Uuid::new_v4());
        store(&key, "first").unwrap();
        assert_eq!(get(&key).unwrap(), "first");

        store(&key, "second").unwrap();
        assert_eq!(get(&key).unwrap(), "second", "the cache has to yield to the newer value");

        delete(&key).unwrap();
        assert!(get(&key).is_err(), "and a deleted secret must not be served from memory either");
    }

    /// The one thing a vault has to do, and the one thing no in-process test
    /// can check by itself: **the secret has to outlive the process**.
    ///
    /// `keyring` 3 puts every platform backend behind a feature flag, and with
    /// none enabled it falls back to an in-memory store. Store-then-read passes.
    /// Everything passes. And every secret is gone at the next launch — which is
    /// how API keys, git tokens and task-source tokens all silently forgot
    /// themselves, while the error blamed the keychain the app had never
    /// written to.
    ///
    /// So this asks a *different process* — macOS's own `security` — whether
    /// the item is really there. With the mock store it isn't, and this fails.
    #[cfg(target_os = "macos")]
    #[test]
    fn secrets_survive_the_process() {
        let _serial = KEYCHAIN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let reference = format!("hangar-vault-test-{}", uuid::Uuid::new_v4());
        store(&reference, "s3cr3t").expect("storing must succeed");

        // Sin `-w`. Pedir el valor obliga a macOS a autorizar la lectura de un
        // ítem creado por otro binario — el de test, cuya firma ad-hoc cambia en
        // cada compilación — y esa autorización falla o se queda esperando de
        // forma impredecible: el test pasaba aislado y fallaba en la suite, con
        // stderr vacío. Buscarlo sin leerlo no necesita autorización.
        //
        // Lo que se pierde: que el valor viaje de vuelta intacto. Lo que se
        // conserva es justo la propiedad que estaba rota — que el ítem **exista
        // en el llavero del sistema** y no en un mapa dentro del proceso.
        let found = std::process::Command::new("security")
            .args(["find-generic-password", "-s", SERVICE, "-a", &reference])
            .output()
            .expect("the `security` CLI ships with macOS");

        let _ = delete(&reference);

        assert!(
            found.status.success(),
            "the secret never reached the real keychain — keyring is on its in-memory store, so every secret dies with the process (exit {:?}: {})",
            found.status.code(),
            String::from_utf8_lossy(&found.stderr).trim()
        );
        // Y que el ítem encontrado es el nuestro, no otro que pasaba por ahí.
        assert!(String::from_utf8_lossy(&found.stdout).contains(&reference));
    }
}
