//! Identidad de esta máquina y el dispositivo emparejado con ella.
//!
//! El emparejamiento *como interfaz* (el QR, aceptar, revocar) es la fase 2. Lo
//! que vive aquí es el sustrato: sin un dispositivo con clave y nivel no hay
//! puente que probar, y probar el puente es lo que la fase 1 tiene que entregar.

use crate::bridge::allowlist::Level;
use crate::bridge::crypto::SharedKey;
use crate::db::DbConnection;
use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use uuid::Uuid;

const MACHINE_ID_KEY: &str = "bridge.machine_id";
const MACHINE_NAME_KEY: &str = "bridge.machine_name";
const DEFAULT_MODE_KEY: &str = "terminal.default_mode";

/// El nombre del canal de este Mac. Un UUID y no el hostname: un hostname es
/// adivinable, cambia al renombrar el equipo, y a menudo lleva el nombre de una
/// persona.
///
/// Se crea la primera vez que alguien lo pide y no cambia nunca — cambiarlo
/// dejaría al teléfono hablando a un canal vacío sin decir por qué.
pub fn machine_id(db: &DbConnection) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let existing: Option<String> = conn
        .query_row("SELECT value FROM app_settings WHERE key = ?1", params![MACHINE_ID_KEY], |row| row.get(0))
        .optional()
        .map_err(|e| e.to_string())?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let id = Uuid::new_v4().to_string();
    conn.execute("INSERT INTO app_settings (key, value) VALUES (?1, ?2)", params![MACHINE_ID_KEY, id]).map_err(|e| e.to_string())?;
    Ok(id)
}

/// Cómo se llama este Mac para una persona. Con varios emparejados, el móvil
/// vería dos UUID si no.
pub fn machine_name(db: &DbConnection) -> String {
    let stored: Option<String> = db
        .0
        .lock()
        .ok()
        .and_then(|conn| conn.query_row("SELECT value FROM app_settings WHERE key = ?1", params![MACHINE_NAME_KEY], |row| row.get(0)).optional().ok().flatten());

    stored.filter(|name| !name.trim().is_empty()).unwrap_or_else(default_machine_name)
}

/// El hostname como valor inicial, porque es el nombre que la persona ya usa
/// para ese ordenador.
fn default_machine_name() -> String {
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "this Mac".to_string())
}

/// El dispositivo emparejado, sin su clave.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedDevice {
    pub id: String,
    pub name: String,
    pub level: Level,
    pub paired_at: String,
    pub last_seen_at: Option<String>,
}

/// Lo que el escritorio necesita saber de sí mismo para enseñarlo y para
/// conectarse.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeStatus {
    pub machine_id: String,
    pub machine_name: String,
    pub device: Option<PairedDevice>,
}

#[tauri::command]
pub fn bridge_status(db: tauri::State<DbConnection>) -> Result<BridgeStatus, String> {
    Ok(BridgeStatus { machine_id: machine_id(&db)?, machine_name: machine_name(&db), device: paired_device(&db) })
}

#[tauri::command]
pub fn set_machine_name(db: tauri::State<DbConnection>, name: String) -> Result<String, String> {
    store_machine_name(&db, &name)
}

/// El cuerpo, sobre una conexión normal: el mismo corte que `account_spend`, y
/// la razón es la misma — un test no debería tener que inventarse un `State`.
fn store_machine_name(db: &DbConnection, name: &str) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![MACHINE_NAME_KEY, name.trim()],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    Ok(machine_name(db))
}

/// El modo con el que nacen las terminales de esta máquina.
///
/// **Global y no por proyecto**, por decisión explícita. La consecuencia hay que
/// tenerla presente: con `yolo`, *todas* las terminales de este Mac ejecutan sin
/// preguntar, incluida la del repositorio que más importa. Lo que protege pasa a
/// ser el worktree aislado y git.
pub fn default_mode(db: &DbConnection) -> crate::providers::Mode {
    let stored: Option<String> = db
        .0
        .lock()
        .ok()
        .and_then(|conn| conn.query_row("SELECT value FROM app_settings WHERE key = ?1", params![DEFAULT_MODE_KEY], |row| row.get(0)).optional().ok().flatten());

    // `ask_edits` por defecto y no `yolo`: un ajuste que no se ha tocado nunca
    // no puede ser el que ejecuta sin preguntar.
    stored.map(|value| crate::providers::Mode::from_str(&value)).unwrap_or(crate::providers::Mode::AskEdits)
}

#[tauri::command]
pub fn get_default_terminal_mode(db: tauri::State<DbConnection>) -> String {
    mode_name(default_mode(&db))
}

#[tauri::command]
pub fn set_default_terminal_mode(db: tauri::State<DbConnection>, mode: String) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO app_settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = ?2",
        params![DEFAULT_MODE_KEY, mode.trim()],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);
    Ok(mode_name(default_mode(&db)))
}

fn mode_name(mode: crate::providers::Mode) -> String {
    match mode {
        crate::providers::Mode::Plan => "plan",
        crate::providers::Mode::AskEdits => "ask_edits",
        crate::providers::Mode::AutoEdit => "auto_edit",
        crate::providers::Mode::Yolo => "yolo",
    }
    .to_string()
}

pub fn paired_device(db: &DbConnection) -> Option<PairedDevice> {
    let conn = db.0.lock().ok()?;
    conn.query_row(
        "SELECT id, name, level, paired_at, last_seen_at FROM paired_device LIMIT 1",
        [],
        |row| {
            let level: String = row.get(2)?;
            Ok(PairedDevice {
                id: row.get(0)?,
                name: row.get(1)?,
                level: serde_json::from_value(serde_json::Value::String(level)).unwrap_or(Level::Watch),
                paired_at: row.get(3)?,
                last_seen_at: row.get(4)?,
            })
        },
    )
    .optional()
    .ok()
    .flatten()
}

/// La clave del dispositivo emparejado. Del llavero, nunca de la base.
pub fn device_key(db: &DbConnection) -> Option<(SharedKey, Level)> {
    let device = paired_device(db)?;
    let key = crate::credential_vault::get(&credential_ref(&device.id)).ok()?;
    Some((SharedKey::from_base64(&key).ok()?, device.level))
}

pub fn credential_ref(device_id: &str) -> String {
    format!("bridge-device-{device_id}")
}

/// Empareja un dispositivo, **expulsando al anterior** si hubiera.
///
/// Un solo teléfono por Mac (`MOBILE_BRIDGE_TRD.md` §12.3). Esa restricción
/// declarada de entrada es lo que elimina el turno exclusivo, su caducidad y la
/// lógica de liberarlo: si nunca puede haber dos, no hay disputa que resolver.
pub fn store_paired_device(db: &DbConnection, name: &str, level: Level, key: &SharedKey) -> Result<PairedDevice, String> {
    if let Some(previous) = paired_device(db) {
        let _ = crate::credential_vault::delete(&credential_ref(&previous.id));
    }

    let id = Uuid::new_v4().to_string();
    crate::credential_vault::store(&credential_ref(&id), &key.to_base64())?;

    let level_name = serde_json::to_value(level).map_err(|e| e.to_string())?.as_str().unwrap_or("watch").to_string();
    let paired_at = chrono::Utc::now().to_rfc3339();
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        // Borrar y insertar, no actualizar: un solo teléfono por Mac, y el
        // anterior deja de existir en el mismo movimiento.
        conn.execute("DELETE FROM paired_device", []).map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO paired_device (id, name, level, credential_ref, paired_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![id, name.trim(), level_name, credential_ref(&id), paired_at],
        )
        .map_err(|e| e.to_string())?;
    }

    Ok(PairedDevice { id, name: name.trim().to_string(), level, paired_at, last_seen_at: None })
}

#[tauri::command]
pub fn bridge_revoke_device(db: tauri::State<DbConnection>) -> Result<(), String> {
    if let Some(device) = paired_device(&db) {
        let _ = crate::credential_vault::delete(&credential_ref(&device.id));
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM paired_device", []).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn touch_last_seen(db: &DbConnection) {
    if let Ok(conn) = db.0.lock() {
        let _ = conn.execute("UPDATE paired_device SET last_seen_at = ?1", params![chrono::Utc::now().to_rfc3339()]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> DbConnection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        crate::db::run_migrations(&conn).unwrap();
        DbConnection(std::sync::Mutex::new(conn))
    }

    /// El id del canal no puede cambiar: el teléfono le habla por nombre, y un
    /// nombre nuevo lo deja hablando a un canal vacío sin decir por qué.
    #[test]
    fn el_id_de_maquina_se_crea_una_vez_y_no_cambia() {
        let db = seeded();
        let first = machine_id(&db).unwrap();
        assert_eq!(machine_id(&db).unwrap(), first);
        assert!(!first.is_empty());
    }

    /// Y no es el hostname, que es adivinable y suele llevar el nombre de una
    /// persona.
    #[test]
    fn el_id_no_es_el_nombre_del_ordenador() {
        let db = seeded();
        assert_ne!(machine_id(&db).unwrap(), machine_name(&db));
    }

    #[test]
    fn el_nombre_se_edita_y_al_vaciarlo_vuelve_al_del_sistema() {
        let db = seeded();
        assert_eq!(store_machine_name(&db, "  Mac de la oficina  ").unwrap(), "Mac de la oficina");
        // Vaciarlo no deja la app sin nombre: vuelve al del sistema.
        assert_eq!(store_machine_name(&db, "   ").unwrap(), default_machine_name());
    }
}
