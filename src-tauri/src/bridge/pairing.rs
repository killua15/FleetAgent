//! Emparejar un teléfono con este Mac.
//!
//! Dos reglas que definen el diseño (`MOBILE_BRIDGE_TRD.md` §7):
//!
//! **La clave viaja en el QR y solo ahí.** Un canal que se mira con los ojos y
//! no atraviesa ningún servidor. Es lo que permite que todo lo demás sea cifrado
//! extremo a extremo sin negociar claves por red.
//!
//! **Escanear no es autorizar.** Aunque el QR sea correcto, el emparejamiento se
//! acepta delante del Mac. Un QR fotografiado por encima del hombro no basta.

use crate::bridge::allowlist::Level;
use crate::bridge::crypto::SharedKey;
use crate::bridge::session;
use crate::db::DbConnection;
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Cuánto vale un QR. Corto a propósito: una captura de pantalla en un chat no
/// debe seguir sirviendo mañana.
const OFFER_TTL: Duration = Duration::from_secs(120);

/// La oferta abierta, si hay una. **En memoria y no en la base**: un QR que
/// sobrevive a cerrar la app es un QR que nadie recuerda haber dejado abierto.
static PENDING: Mutex<Option<Pending>> = Mutex::new(None);

struct Pending {
    key: SharedKey,
    level: Level,
    created: Instant,
}

/// Lo que se pinta en el QR.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingOffer {
    pub machine_id: String,
    pub machine_name: String,
    /// La clave, en base64. Sale de Rust una sola vez, para dibujarla.
    pub key: String,
    pub level: Level,
    /// Segundos que le quedan, para que la interfaz pueda decirlo.
    pub expires_in: u64,
}

/// Abre una oferta y devuelve lo que hay que dibujar.
///
/// Cada llamada **sustituye** la anterior: dos QR válidos a la vez significan
/// dos claves vivas para un solo teléfono, y solo una acabaría usándose.
#[tauri::command]
pub fn bridge_begin_pairing(db: tauri::State<DbConnection>, level: Level) -> Result<PairingOffer, String> {
    let key = SharedKey::generate();
    let offer = PairingOffer {
        machine_id: session::machine_id(&db)?,
        machine_name: session::machine_name(&db),
        key: key.to_base64(),
        level,
        expires_in: OFFER_TTL.as_secs(),
    };

    let mut pending = PENDING.lock().map_err(|e| e.to_string())?;
    *pending = Some(Pending { key, level, created: Instant::now() });
    Ok(offer)
}

#[tauri::command]
pub fn bridge_cancel_pairing() -> Result<(), String> {
    let mut pending = PENDING.lock().map_err(|e| e.to_string())?;
    *pending = None;
    Ok(())
}

/// La clave de la oferta abierta, si no ha caducado.
///
/// Caducar aquí y no con un temporizador: un temporizador que no se dispara
/// deja la oferta viva, y comprobar la hora al usarla no puede fallar.
pub fn pending_key() -> Option<(SharedKey, Level)> {
    let mut pending = PENDING.lock().ok()?;
    let expired = pending.as_ref().is_some_and(|p| p.created.elapsed() > OFFER_TTL);
    if expired {
        *pending = None;
        return None;
    }
    pending.as_ref().map(|p| (p.key.clone(), p.level))
}

/// Un teléfono escaneó el QR y se ha presentado. **No queda emparejado**: esto
/// solo se lo cuenta al escritorio para que alguien decida.
pub fn announce_request<R: Runtime>(app: &AppHandle<R>, device_name: &str) {
    let _ = app.emit("bridge-pairing-request", serde_json::json!({ "device": device_name }));
}

/// Acepta la oferta abierta y guarda el dispositivo, expulsando al anterior.
#[tauri::command]
pub fn bridge_accept_pairing<R: Runtime>(app: AppHandle<R>, name: String) -> Result<session::PairedDevice, String> {
    let (key, level) = pending_key().ok_or("no hay ninguna solicitud de emparejamiento abierta (¿caducó el QR?)")?;
    let db = app.state::<DbConnection>();

    let device = session::store_paired_device(&db, &name, level, &key)?;
    bridge_cancel_pairing()?;
    // El puente arranca solo en cuanto hay con quién hablar.
    let _ = app.emit("bridge-device-changed", serde_json::json!({ "paired": true }));
    Ok(device)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sin oferta abierta no hay clave con la que descifrar a un desconocido.
    #[test]
    fn sin_oferta_no_hay_clave() {
        let _ = bridge_cancel_pairing();
        assert!(pending_key().is_none());
    }

    /// Dos QR válidos a la vez serían dos claves vivas para un solo teléfono.
    #[test]
    fn una_oferta_nueva_sustituye_a_la_anterior() {
        let first = SharedKey::generate();
        {
            let mut pending = PENDING.lock().unwrap();
            *pending = Some(Pending { key: first.clone(), level: Level::Watch, created: Instant::now() });
        }
        let second = SharedKey::generate();
        {
            let mut pending = PENDING.lock().unwrap();
            *pending = Some(Pending { key: second.clone(), level: Level::Steer, created: Instant::now() });
        }

        let (key, level) = pending_key().unwrap();
        assert_eq!(key.to_base64(), second.to_base64());
        assert_ne!(key.to_base64(), first.to_base64());
        assert_eq!(level, Level::Steer);
        let _ = bridge_cancel_pairing();
    }

    /// Una captura de pantalla de ayer no vale. Se comprueba al usarla, no con
    /// un temporizador: un temporizador que no salta deja la oferta viva.
    #[test]
    fn una_oferta_caducada_no_sirve() {
        {
            let mut pending = PENDING.lock().unwrap();
            *pending = Some(Pending {
                key: SharedKey::generate(),
                level: Level::Launch,
                created: Instant::now() - OFFER_TTL - Duration::from_secs(1),
            });
        }
        assert!(pending_key().is_none(), "un QR caducado no puede emparejar");
        // Y se limpia al detectarlo, no se queda ocupando sitio.
        assert!(PENDING.lock().unwrap().is_none());
    }
}
