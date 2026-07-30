use keyring::Entry;
use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

/// Keychain service name. Renamed with the product; no migration from the
/// pre-rename `com.killua.fleet` service is needed or included — that service
/// was verified to hold no entries at rename time (both existing accounts are
/// subscription-billed, and only API-key accounts ever store a secret here).
/// An API key that predated the rename would need re-entering, nothing worse.
const SERVICE: &str = "com.killua.hangar";

/// El llavero al que se habla, distinto por directorio de datos.
///
/// **El llavero no vive en el directorio de datos**, así que separar la base de
/// datos no separaba los secretos. Y uno de ellos no tolera compañía: el refresh
/// token de Supabase **rota** en cada uso. Con las dos instancias sobre la misma
/// entrada, la primera en arrancar lo rota y la segunda intenta uno ya retirado,
/// falla, y al fallar **borra la entrada compartida** — dejando sin sesión
/// guardada también a la instancia que funcionaba. Pasó de verdad.
///
/// Con `HANGAR_DATA_DIR` puesto, el servicio lleva un sufijo derivado de la ruta:
/// eso aísla el llavero entero de una vez, en lugar de ir parcheando claves
/// sueltas cada vez que se añade un secreto nuevo.
fn service() -> &'static str {
    static RESOLVED: OnceLock<String> = OnceLock::new();
    RESOLVED.get_or_init(|| match std::env::var(crate::app_data::DATA_DIR_ENV) {
        Ok(dir) => format!("{SERVICE}.{}", namespace_suffix(&dir)),
        Err(_) => SERVICE.to_string(),
    })
}

/// Sufijo estable para una ruta.
///
/// FNV-1a a mano y no `DefaultHasher`: este último no garantiza el mismo
/// resultado entre versiones de Rust, y un sufijo que cambia al actualizar el
/// compilador deja los secretos guardados en un servicio que nadie vuelve a
/// mirar — un "no encuentro la credencial" sin causa visible.
fn namespace_suffix(dir: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in dir.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Credentials never touch the SQLite database or the frontend — only a
/// `credential_ref` (the account id) is stored there. The actual secret
/// (API key or CLI session token) lives in the OS keychain.
fn entry(credential_ref: &str) -> Result<Entry, String> {
    Entry::new(service(), credential_ref).map_err(|e| e.to_string())
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

    /// El sufijo es estable: el mismo directorio da siempre el mismo servicio, o
    /// los secretos guardados quedan en un llavero que nadie vuelve a mirar.
    #[test]
    fn el_sufijo_del_llavero_es_estable_por_ruta() {
        assert_eq!(namespace_suffix("/Users/kate/.hangar-dev"), namespace_suffix("/Users/kate/.hangar-dev"));
        assert_ne!(namespace_suffix("/Users/kate/.hangar-dev"), namespace_suffix("/Users/kate/.hangar-otro"));
        assert_eq!(namespace_suffix("/Users/kate/.hangar-dev").len(), 16);
    }
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
            .args(["find-generic-password", "-s", service(), "-a", &reference])
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
