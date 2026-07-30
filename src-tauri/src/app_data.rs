//! One-time move of the app data directory across the Fleet → Hangar rename.
//!
//! Tauri derives `app_data_dir()` from `identifier` in `tauri.conf.json`, so
//! renaming the product moved the directory the app reads out from under it.
//! Everything stateful lives in there: the SQLite database, every account's
//! isolated `CLAUDE_CONFIG_DIR` (`accounts/<id>`, holding real OAuth logins),
//! in-flight logins (`pending-logins/`), and composer attachments. Without
//! this, the rename would present exactly like a factory reset — the app
//! starts with no projects, no sessions and no signed-in accounts, while the
//! real data sits in a directory nothing opens anymore.
//!
//! Called at the very top of `setup`, before `db::init` — the first thing
//! that touches (and would create) the directory. It no-ops on every launch
//! after the first, and on any machine that never ran a pre-rename build.
//!
//! The one rule here: **never clobber**. If both directories hold data, the
//! post-rename one wins and the legacy one is left untouched for the user to
//! deal with, because guessing wrong destroys account logins that can only be
//! recovered by signing in again.

use std::fs;
use std::path::{Path, PathBuf};

/// App data directory of the pre-rename build (`identifier` was
/// `com.killua.fleet`). Always a sibling of the current one — Tauri puts every
/// app's data dir under the same platform-specific parent — which is how the
/// legacy path is derived without hardcoding a whole absolute path per OS.
const LEGACY_DIR_NAME: &str = "com.killua.fleet";

/// Database filename before the rename.
const LEGACY_DB_FILE: &str = "fleet.db";

/// Un directorio de datos alternativo, para poder correr una versión de
/// desarrollo **al lado** de la estable sin que se pisen.
///
/// Sin esto, las dos comparten base de datos y llavero: dos procesos escribiendo
/// el mismo SQLite, dos registros de terminales y dos puentes en el mismo canal.
/// La base en sí es compatible hacia atrás —una migración nueva solo añade, y un
/// build viejo salta las que no conoce— así que el problema no es el formato,
/// es la concurrencia.
///
///     HANGAR_DATA_DIR=~/.hangar-dev npm run tauri dev
///
/// Solo se lee del entorno, así que un build instalado nunca lo tiene puesto.
pub const DATA_DIR_ENV: &str = "HANGAR_DATA_DIR";

/// El directorio de datos que toca usar: el del entorno si está, y si no el que
/// Tauri deriva del identificador.
pub fn root<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, String> {
    use tauri::Manager;
    let fallback = || app.path().app_data_dir().map_err(|e| format!("could not resolve the app data directory: {e}"));
    match std::env::var(DATA_DIR_ENV) {
        Ok(raw) => Ok(expand_home(&raw, std::env::var("HOME").ok().as_deref())),
        Err(_) => fallback(),
    }
}

/// `~/` es lo que uno teclea, y `PathBuf` no lo expande: sin esto acabas con un
/// directorio llamado literalmente `~` dentro del repo, difícil de ver en un `ls`.
///
/// Aparte, es la parte comprobable de `root` sin tocar el entorno del proceso
/// —que es global y compartido con los demás tests corriendo en paralelo.
fn expand_home(raw: &str, home: Option<&str>) -> PathBuf {
    match (raw.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(raw),
    }
}

/// Database filename now. `db::init` opens this.
pub(crate) const DB_FILE: &str = "hangar.db";

/// SQLite's sidecars. Not present in the journal mode this app runs in
/// (`DELETE`, the default — no persistent `-wal`/`-shm`), but moved anyway:
/// leaving a stale sidecar next to a renamed database is the kind of thing
/// that only shows up as corruption much later, and it costs three lines.
const DB_SIDECAR_SUFFIXES: &[&str] = &["-wal", "-shm", "-journal"];

/// Resolves the current app data dir, derives the legacy one as its sibling,
/// and migrates. Failures are logged, never fatal: a launch that can't
/// migrate is still a launch the user can sign in to, whereas a panic here
/// bricks the app entirely.
pub(crate) fn migrate<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    use tauri::Manager;

    // Con un directorio propio no hay nada que renombrar: la ruta legada se
    // deriva como *hermana* de la que da Tauri, y la hermana de `~/.hangar-dev`
    // no es el directorio de nadie. Peor: mover los datos reales de la versión
    // estable hacia una instancia de pruebas es exactamente el estropicio que
    // el override existe para evitar.
    if std::env::var_os(DATA_DIR_ENV).is_some() {
        log::info!("app data migration: skipped, {DATA_DIR_ENV} is set");
        return;
    }

    let Ok(current) = app.path().app_data_dir() else {
        log::warn!("app data migration: could not resolve the app data dir; skipping");
        return;
    };
    let Some(legacy) = current.parent().map(|p| p.join(LEGACY_DIR_NAME)) else {
        return;
    };
    if let Err(e) = migrate_dir(&legacy, &current) {
        log::error!("app data migration from {} failed: {e}", legacy.display());
    }
}

/// Whether `dir` holds anything at all. An existing-but-empty directory is
/// treated as absent: something other than the app (or an aborted earlier
/// launch) can create it, and skipping the migration because of an empty
/// folder is how the user would end up staring at an empty app with their
/// data one directory over.
fn is_empty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(mut entries) => entries.next().is_none(),
        // Unreadable is not empty — err toward leaving it alone.
        Err(_) => false,
    }
}

/// The whole migration, as a pure two-path operation so it can be tested
/// against real temp directories without a Tauri app.
pub(crate) fn migrate_dir(legacy: &Path, current: &Path) -> Result<(), String> {
    if !legacy.is_dir() || is_empty(legacy) {
        // Nothing to bring over. The database still needs its new name on a
        // machine that already migrated the directory but not the file (or
        // one where the dir was moved by hand).
        return rename_db_file(current);
    }

    if !current.exists() {
        if let Some(parent) = current.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
        }
        // `rename` is atomic and instant within a volume, which matters:
        // there is no window where half the accounts are in one place and
        // half in the other. It only fails across volumes, hence the copy.
        match fs::rename(legacy, current) {
            Ok(()) => log::info!("app data migrated: {} → {}", legacy.display(), current.display()),
            Err(_) => {
                copy_dir(legacy, current)?;
                log::info!("app data copied (cross-volume): {} → {}; the old directory was left in place", legacy.display(), current.display());
            }
        }
    } else if is_empty(current) {
        // The destination exists but is inert. Move the contents across
        // rather than skipping, then drop the husk.
        for entry in fs::read_dir(legacy).map_err(|e| format!("read {}: {e}", legacy.display()))? {
            let entry = entry.map_err(|e| format!("read entry in {}: {e}", legacy.display()))?;
            let to = current.join(entry.file_name());
            if fs::rename(entry.path(), &to).is_err() {
                copy_path(&entry.path(), &to)?;
            }
        }
        let _ = fs::remove_dir(legacy);
        log::info!("app data moved into existing empty dir: {} → {}", legacy.display(), current.display());
    } else {
        // Both hold data. Never guess.
        log::warn!(
            "app data migration skipped: both {} and {} hold data — leaving the old one untouched",
            legacy.display(),
            current.display()
        );
        return rename_db_file(current);
    }

    rename_db_file(current)
}

/// `fleet.db` → `hangar.db`, sidecars included. Idempotent, and a no-op once
/// the new name exists (which also means a leftover legacy file next to a
/// live database is never allowed to overwrite it).
fn rename_db_file(dir: &Path) -> Result<(), String> {
    let legacy_db = dir.join(LEGACY_DB_FILE);
    let new_db = dir.join(DB_FILE);
    if !legacy_db.is_file() || new_db.exists() {
        return Ok(());
    }
    fs::rename(&legacy_db, &new_db).map_err(|e| format!("rename {} → {}: {e}", legacy_db.display(), new_db.display()))?;
    for suffix in DB_SIDECAR_SUFFIXES {
        let from = dir.join(format!("{LEGACY_DB_FILE}{suffix}"));
        let to = dir.join(format!("{DB_FILE}{suffix}"));
        if from.is_file() && !to.exists() {
            let _ = fs::rename(from, to);
        }
    }
    log::info!("database renamed: {LEGACY_DB_FILE} → {DB_FILE} in {}", dir.display());
    Ok(())
}

fn copy_path(from: &Path, to: &PathBuf) -> Result<(), String> {
    if from.is_dir() {
        copy_dir(from, to)
    } else {
        fs::copy(from, to).map(|_| ()).map_err(|e| format!("copy {} → {}: {e}", from.display(), to.display()))
    }
}

/// Recursive directory copy — only reached on a cross-volume move, which
/// `fs::rename` can't do. Deliberately does not delete the source: if a copy
/// half-fails, the original is the only remaining record of the user's
/// account logins.
fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(|e| format!("create {}: {e}", to.display()))?;
    for entry in fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))? {
        let entry = entry.map_err(|e| format!("read entry in {}: {e}", from.display()))?;
        copy_path(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

#[cfg(test)]
mod override_tests {
    use super::*;

    /// La virgulilla se expande. Es lo que uno teclea, y un directorio llamado
    /// literalmente "~" es de esos despistes que no se ven hasta que faltan las
    /// cuentas.
    #[test]
    fn la_virgulilla_se_expande_al_home() {
        assert_eq!(expand_home("~/.hangar-dev", Some("/Users/kate")), PathBuf::from("/Users/kate/.hangar-dev"));
    }

    /// Una ruta absoluta pasa tal cual, y una con "~" en medio no es un home:
    /// solo el prefijo cuenta.
    #[test]
    fn una_ruta_absoluta_no_se_toca() {
        assert_eq!(expand_home("/tmp/hangar-dev", Some("/Users/kate")), PathBuf::from("/tmp/hangar-dev"));
        assert_eq!(expand_home("/opt/a~b", Some("/Users/kate")), PathBuf::from("/opt/a~b"));
    }

    /// Sin HOME —cron, un daemon— se usa lo que se pidió en vez de fabricar una
    /// ruta a partir de una cadena vacía, que apuntaría a "/.hangar-dev".
    #[test]
    fn sin_home_se_respeta_lo_pedido() {
        assert_eq!(expand_home("~/.hangar-dev", None), PathBuf::from("~/.hangar-dev"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic pre-rename directory: the database plus one account's
    /// config dir with a credentials file in it.
    fn legacy_fixture(root: &Path) -> PathBuf {
        let legacy = root.join(LEGACY_DIR_NAME);
        fs::create_dir_all(legacy.join("accounts").join("acc-1")).unwrap();
        fs::write(legacy.join(LEGACY_DB_FILE), b"sqlite-bytes").unwrap();
        fs::write(legacy.join("accounts").join("acc-1").join(".credentials.json"), b"{}").unwrap();
        legacy
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("hangar-appdata-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn legacy_dir_moves_wholesale_and_the_db_is_renamed() {
        let root = temp_root("move");
        let legacy = legacy_fixture(&root);
        let current = root.join("com.killua.hangar");

        migrate_dir(&legacy, &current).unwrap();

        assert!(!legacy.exists(), "the legacy directory must be gone after a same-volume move");
        assert_eq!(fs::read_to_string(current.join(DB_FILE)).unwrap(), "sqlite-bytes");
        assert!(!current.join(LEGACY_DB_FILE).exists(), "the old database name must not linger");
        assert!(
            current.join("accounts").join("acc-1").join(".credentials.json").is_file(),
            "an account's config dir (its real CLI login) must survive the move"
        );
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_existing_empty_current_dir_is_filled_instead_of_skipped() {
        let root = temp_root("empty-target");
        let legacy = legacy_fixture(&root);
        let current = root.join("com.killua.hangar");
        fs::create_dir_all(&current).unwrap();

        migrate_dir(&legacy, &current).unwrap();

        assert_eq!(fs::read_to_string(current.join(DB_FILE)).unwrap(), "sqlite-bytes");
        assert!(current.join("accounts").join("acc-1").is_dir());
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn data_already_under_the_new_identifier_is_never_clobbered() {
        let root = temp_root("no-clobber");
        let legacy = legacy_fixture(&root);
        let current = root.join("com.killua.hangar");
        fs::create_dir_all(&current).unwrap();
        fs::write(current.join(DB_FILE), b"the-real-database").unwrap();

        migrate_dir(&legacy, &current).unwrap();

        assert_eq!(
            fs::read_to_string(current.join(DB_FILE)).unwrap(),
            "the-real-database",
            "a database created after the rename must win over the legacy one"
        );
        assert!(legacy.join(LEGACY_DB_FILE).is_file(), "the legacy directory must be left intact, not half-consumed");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_db_file_is_renamed_even_with_no_legacy_dir_to_move() {
        let root = temp_root("db-only");
        let legacy = root.join(LEGACY_DIR_NAME);
        let current = root.join("com.killua.hangar");
        fs::create_dir_all(&current).unwrap();
        fs::write(current.join(LEGACY_DB_FILE), b"sqlite-bytes").unwrap();
        fs::write(current.join(format!("{LEGACY_DB_FILE}-wal")), b"wal").unwrap();

        migrate_dir(&legacy, &current).unwrap();

        assert_eq!(fs::read_to_string(current.join(DB_FILE)).unwrap(), "sqlite-bytes");
        assert!(current.join(format!("{DB_FILE}-wal")).is_file(), "sidecars must follow the database, not be orphaned");
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_fresh_install_with_nothing_to_migrate_is_a_no_op() {
        let root = temp_root("fresh");
        let legacy = root.join(LEGACY_DIR_NAME);
        let current = root.join("com.killua.hangar");

        migrate_dir(&legacy, &current).unwrap();

        assert!(!current.exists(), "migration must not create the app data dir — db::init owns that");
        fs::remove_dir_all(&root).ok();
    }
}
