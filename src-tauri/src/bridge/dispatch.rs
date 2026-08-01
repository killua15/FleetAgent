//! Lo que el webview puede preguntarle a Rust sobre un mensaje del puente.
//!
//! **Aquí está la razón de que el escritorio no haya que volver a tocarlo.** El
//! webview no sabe qué transporta: le pasa el sobre a Rust, Rust lo descifra, lo
//! valida y le devuelve *solo el nombre y los argumentos* si está permitido. El
//! webview entonces llama a `invoke(nombre, args)` — el mismo `invoke` que usa
//! la app de escritorio — así que **no hay tabla de despacho que mantener**: un
//! comando nuevo llega al móvil añadiendo una línea a `allowlist.json`.
//!
//! El orden de esa frase importa: descifrar y autorizar pasan **en Rust, antes**
//! de que el nombre llegue al webview. Al revés, el webview podría invocar
//! cualquier cosa.

use crate::bridge::protocol::{Envelope, ErrorCode, Failure, Hello, Kind, PROTOCOL_VERSION};
use crate::bridge::{allowlist, crypto, session, Incoming};
use crate::db::DbConnection;
use serde::Serialize;

/// Qué debe hacer el webview con el mensaje que acaba de llegar.
#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum Decision {
    /// Invocar esto. Versión, cifrado y permiso ya están resueltos.
    Invoke { id: String, name: String, args: serde_json::Value },
    /// Publicar este sobre tal cual y no hacer nada más.
    Reply { envelope: Envelope },
    /// Ignorar. No hay dispositivo emparejado, así que no hay a quién
    /// contestar — ni siquiera un error, porque cifrarlo requeriría la clave
    /// que no existe.
    Ignore,
}

/// Punto de entrada del puente. Todo mensaje entrante pasa por aquí.
#[tauri::command]
pub fn bridge_receive<R: tauri::Runtime>(app: tauri::AppHandle<R>, envelope: Envelope) -> Result<Decision, String> {
    use tauri::Manager;
    let db = app.state::<DbConnection>();

    let Some((key, level)) = session::device_key(&db) else {
        // Nadie emparejado. Pero si hay un QR abierto, este mensaje puede ser
        // el teléfono que acaba de escanearlo — y solo lo sabremos si descifra
        // con la clave de la oferta, que es la prueba de que vio el QR.
        return Ok(greet_or_ignore(&app, &envelope));
    };
    session::touch_last_seen(&db);

    match crate::bridge::handle_incoming(&envelope, &key, level) {
        Incoming::Run { id, invoke } => Ok(Decision::Invoke { id, name: invoke.name, args: invoke.args }),

        Incoming::Greet { id, level } => {
            let hello = Hello {
                v: PROTOCOL_VERSION,
                device: session::paired_device(&db).map(|d| d.name).unwrap_or_default(),
                // Lo que este dispositivo puede pedir, para que la app no
                // enseñe botones que van a fallar.
                commands: allowlist::commands_for(level),
                machine_name: Some(session::machine_name(&db)),
            };
            let envelope = crate::bridge::outgoing(Kind::Hello, id, &key, &hello)?;
            Ok(Decision::Reply { envelope })
        }

        Incoming::Reject { id, failure } => {
            let envelope = crate::bridge::outgoing(Kind::Error, id, &key, &failure)?;
            Ok(Decision::Reply { envelope })
        }
    }
}

/// Un mensaje sin dispositivo emparejado: o es el teléfono que acaba de
/// escanear el QR, o es ruido.
///
/// Descifrar con la clave de la oferta **es** la comprobación de que vio el QR:
/// nadie más la tiene, porque nunca salió de la pantalla. Y aun así no queda
/// emparejado — se avisa al escritorio y se le dice al teléfono que espere.
fn greet_or_ignore<R: tauri::Runtime>(app: &tauri::AppHandle<R>, envelope: &Envelope) -> Decision {
    let Some((key, _)) = crate::bridge::pairing::pending_key() else { return Decision::Ignore };
    let Ok(plaintext) = crypto::open(&key, &envelope.nonce, &envelope.payload) else {
        // La oferta está abierta pero esto no viene de quien vio el QR.
        return Decision::Ignore;
    };

    let device = serde_json::from_slice::<Hello>(&plaintext).map(|hello| hello.device).unwrap_or_else(|_| "un dispositivo".to_string());
    crate::bridge::pairing::announce_request(app, &device);

    let failure = Failure::new(ErrorCode::PendingApproval, "esperando que se acepte en el Mac");
    match crate::bridge::outgoing(Kind::Error, envelope.id.clone(), &key, &failure) {
        Ok(envelope) => Decision::Reply { envelope },
        Err(_) => Decision::Ignore,
    }
}

/// El resultado de un comando, camino de vuelta.
///
/// El webview manda lo que devolvió `invoke` sin interpretarlo: Rust lo cifra.
/// Que el resultado no pase por manos del webview *en claro hacia el canal* es
/// lo que mantiene el cifrado extremo a extremo con el transporte en JS.
#[tauri::command]
pub fn bridge_reply(db: tauri::State<DbConnection>, id: String, ok: bool, data: serde_json::Value) -> Result<Envelope, String> {
    let Some((key, _)) = session::device_key(&db) else {
        return Err("no hay dispositivo emparejado".to_string());
    };

    if ok {
        let body = serde_json::json!({ "ok": true, "data": data });
        return sealed_or_too_large(&key, Kind::Result, id, &body);
    }

    // Un comando que devolvió `Err` no es un fallo del puente: su mensaje es lo
    // único útil que hay, y va tal cual.
    let failure = Failure::new(ErrorCode::CommandFailed, data.as_str().unwrap_or("el comando falló").to_string());
    crate::bridge::outgoing(Kind::Error, id, &key, &failure)
}

/// Un evento de Hangar, camino del móvil.
#[tauri::command]
pub fn bridge_event(db: tauri::State<DbConnection>, name: String, data: serde_json::Value) -> Result<Option<Envelope>, String> {
    let Some((key, _)) = session::device_key(&db) else {
        return Ok(None);
    };
    let event = crate::bridge::protocol::Event { name, data };
    Ok(Some(crate::bridge::outgoing(Kind::Event, uuid::Uuid::new_v4().to_string(), &key, &event)?))
}

/// El canal de Supabase corta los mensajes grandes, y un sobre recortado llega
/// como basura indescifrable. Mejor un error que dice qué pasó.
const MAX_PAYLOAD_BYTES: usize = 200 * 1024;

fn sealed_or_too_large(key: &crypto::SharedKey, kind: Kind, id: String, body: &impl Serialize) -> Result<Envelope, String> {
    let envelope = crate::bridge::outgoing(kind, id.clone(), key, body)?;
    if envelope.payload.len() <= MAX_PAYLOAD_BYTES {
        return Ok(envelope);
    }
    let failure = Failure::new(
        ErrorCode::TooLarge,
        format!("la respuesta ocupa {} KB y el canal admite {} KB", envelope.payload.len() / 1024, MAX_PAYLOAD_BYTES / 1024),
    );
    crate::bridge::outgoing(Kind::Error, id, key, &failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El límite del canal es real y un sobre recortado llega como basura
    /// indescifrable — que es exactamente el fallo que nadie sabe diagnosticar.
    #[test]
    fn una_respuesta_demasiado_grande_se_convierte_en_error() {
        let key = crypto::SharedKey::generate();
        let enorme = serde_json::json!({ "data": "x".repeat(MAX_PAYLOAD_BYTES) });

        let envelope = sealed_or_too_large(&key, Kind::Result, "01J".into(), &enorme).unwrap();
        assert_eq!(envelope.kind, Kind::Error);

        let plaintext = crypto::open(&key, &envelope.nonce, &envelope.payload).unwrap();
        let failure: Failure = serde_json::from_slice(&plaintext).unwrap();
        assert_eq!(failure.code, ErrorCode::TooLarge);
        assert!(failure.message.contains("KB"), "el error tiene que decir cuánto sobra: {}", failure.message);
    }

    /// Y una normal pasa sin tocarse.
    #[test]
    fn una_respuesta_normal_viaja_como_resultado() {
        let key = crypto::SharedKey::generate();
        let envelope = sealed_or_too_large(&key, Kind::Result, "01J".into(), &serde_json::json!({ "ok": true })).unwrap();
        assert_eq!(envelope.kind, Kind::Result);
    }
}
