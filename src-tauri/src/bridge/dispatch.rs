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
use crate::bridge::allowlist::Level;
use crate::bridge::{allowlist, crypto, peers, session, Incoming};
use crate::db::DbConnection;
use serde::Serialize;

/// Qué debe hacer el webview con el mensaje que acaba de llegar.
#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum Decision {
    /// Invocar esto. Versión, cifrado y permiso ya están resueltos.
    ///
    /// `peer` is the machine that asked, when it was not the phone: the reply
    /// has to be sealed for it, so the webview hands it back to `bridge_reply`.
    Invoke {
        id: String,
        name: String,
        args: serde_json::Value,
        #[serde(skip_serializing_if = "Option::is_none")]
        peer: Option<String>,
    },
    /// Publicar este sobre tal cual y no hacer nada más.
    Reply { envelope: Envelope },
    /// Ignorar. No hay dispositivo emparejado, así que no hay a quién
    /// contestar — ni siquiera un error, porque cifrarlo requeriría la clave
    /// que no existe.
    Ignore,
    /// From a machine this one has no key for yet — possibly one that only just
    /// published it. The webview re-reads the account's machines and hands the
    /// envelope back once; if the sender is still unknown, it is ignored.
    RefreshPeers,
}

/// Punto de entrada del puente. Todo mensaje entrante pasa por aquí.
#[tauri::command]
pub fn bridge_receive<R: tauri::Runtime>(app: tauri::AppHandle<R>, envelope: Envelope) -> Result<Decision, String> {
    use tauri::Manager;
    let db = app.state::<DbConnection>();

    // Another machine of the account. Routed before the phone's path, which
    // never sets `from`, so the phone is untouched by any of this.
    if let Some(from) = envelope.from.clone() {
        return Ok(receive_from_peer(&db, &envelope, &from));
    }

    let Some((key, level)) = session::device_key(&db) else {
        // Nadie emparejado. Pero si hay un QR abierto, este mensaje puede ser
        // el teléfono que acaba de escanearlo — y solo lo sabremos si descifra
        // con la clave de la oferta, que es la prueba de que vio el QR.
        return Ok(greet_or_ignore(&app, &envelope));
    };
    session::touch_last_seen(&db);

    match crate::bridge::handle_incoming(&envelope, &key, level) {
        Incoming::Run { id, invoke } => Ok(Decision::Invoke { id, name: invoke.name, args: invoke.args, peer: None }),

        Incoming::Greet { id, level } => {
            let hello = Hello {
                v: PROTOCOL_VERSION,
                device: session::paired_device(&db).map(|d| d.name).unwrap_or_default(),
                // Lo que este dispositivo puede pedir, para que la app no
                // enseñe botones que van a fallar.
                commands: allowlist::commands_for(level),
                machine_name: Some(session::machine_name(&db)),
                remote_access: None,
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

/// A message from one of the account's other machines.
///
/// Silence for anything this machine cannot answer securely — addressed to
/// someone else, or from a machine it has no key for — exactly like the
/// phone's path: answering an unauthenticated message is answering anyone.
///
/// With remote access off it still greets (so the other side can say "turned
/// off there" rather than "offline") but runs nothing.
fn receive_from_peer(db: &DbConnection, envelope: &Envelope, from: &str) -> Decision {
    let Ok(own_id) = session::machine_id(db) else { return Decision::Ignore };
    if envelope.to.as_deref() != Some(own_id.as_str()) {
        return Decision::Ignore;
    }
    let Some(key) = peers::key_for(from) else { return Decision::RefreshPeers };
    let enabled = peers::remote_access_enabled(db);
    let reply = |kind: Kind, id: String, body: &dyn erased::Body| -> Decision {
        match body.seal(kind, id, &key, &own_id, from) {
            Ok(envelope) => Decision::Reply { envelope },
            Err(_) => Decision::Ignore,
        }
    };

    match crate::bridge::handle_incoming(envelope, &key, Level::Machine) {
        Incoming::Greet { id, .. } => {
            peers::mark_seen(from);
            let hello = Hello {
                v: PROTOCOL_VERSION,
                device: session::machine_name(db),
                commands: if enabled { allowlist::commands_for(Level::Machine) } else { Vec::new() },
                machine_name: Some(session::machine_name(db)),
                remote_access: Some(enabled),
            };
            reply(Kind::Hello, id, &hello)
        }
        Incoming::Run { id, mut invoke } => {
            peers::mark_seen(from);
            if !enabled {
                let failure = Failure::new(ErrorCode::NotAllowed, "el acceso remoto está desactivado en esta máquina");
                return reply(Kind::Error, id, &failure);
            }
            // The `remote_*` terminal commands act on behalf of a machine. Which
            // one is decided here, from the authenticated sender, never from the
            // arguments it sent.
            if invoke.name.starts_with("remote_") {
                if let Some(args) = invoke.args.as_object_mut() {
                    args.insert("peer".to_string(), serde_json::Value::String(from.to_string()));
                } else {
                    invoke.args = serde_json::json!({ "peer": from });
                }
            }
            Decision::Invoke { id, name: invoke.name, args: invoke.args, peer: Some(from.to_string()) }
        }
        Incoming::Reject { id, failure } => reply(Kind::Error, id, &failure),
    }
}

/// `reply` above takes bodies of different types; this keeps it one closure.
mod erased {
    use crate::bridge::crypto::SharedKey;
    use crate::bridge::protocol::{Envelope, Kind};

    pub trait Body {
        fn seal(&self, kind: Kind, id: String, key: &SharedKey, from: &str, to: &str) -> Result<Envelope, String>;
    }

    impl<T: serde::Serialize> Body for T {
        fn seal(&self, kind: Kind, id: String, key: &SharedKey, from: &str, to: &str) -> Result<Envelope, String> {
            crate::bridge::outgoing_to_peer(kind, id, key, self, from, to)
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
///
/// `peer` is what `Decision::Invoke` said: absent for the phone.
#[tauri::command]
pub fn bridge_reply(db: tauri::State<DbConnection>, id: String, ok: bool, data: serde_json::Value, peer: Option<String>) -> Result<Envelope, String> {
    let (key, route) = match peer {
        Some(peer) => {
            let key = peers::key_for(&peer).ok_or_else(|| format!("la máquina {peer} ya no es de esta cuenta"))?;
            (key, Some((session::machine_id(&db)?, peer)))
        }
        None => {
            let Some((key, _)) = session::device_key(&db) else {
                return Err("no hay dispositivo emparejado".to_string());
            };
            (key, None)
        }
    };

    let mut envelope = if ok {
        let body = serde_json::json!({ "ok": true, "data": data });
        sealed_or_too_large(&key, Kind::Result, id, &body)?
    } else {
        // Un comando que devolvió `Err` no es un fallo del puente: su mensaje es lo
        // único útil que hay, y va tal cual.
        let failure = Failure::new(ErrorCode::CommandFailed, data.as_str().unwrap_or("el comando falló").to_string());
        crate::bridge::outgoing(Kind::Error, id, &key, &failure)?
    };
    if let Some((from, to)) = route {
        envelope.from = Some(from);
        envelope.to = Some(to);
    }
    Ok(envelope)
}

/// The event that carries raw PTY bytes. It goes only to the machine that
/// attached the terminal, and never to the phone.
pub const REMOTE_TERMINAL_OUTPUT: &str = "remote-terminal-output";

/// Un evento de Hangar, camino del móvil y de las máquinas conectadas.
///
/// One envelope per recipient, each sealed with that recipient's key: the
/// phone (if paired) and every machine that greeted within `PEER_TTL`, as long
/// as remote access is on.
#[tauri::command]
pub fn bridge_event(db: tauri::State<DbConnection>, name: String, data: serde_json::Value) -> Result<Vec<Envelope>, String> {
    let mut out = Vec::new();

    if name == REMOTE_TERMINAL_OUTPUT {
        // Addressed: the raw stream never fans out.
        let Some(peer) = data.get("peer").and_then(|p| p.as_str()).map(str::to_string) else { return Ok(out) };
        if !peers::remote_access_enabled(&db) || !peers::is_connected(&peer) {
            return Ok(out);
        }
        let Some(key) = peers::key_for(&peer) else { return Ok(out) };
        let event = crate::bridge::protocol::Event { name, data };
        out.push(crate::bridge::outgoing_to_peer(Kind::Event, uuid::Uuid::new_v4().to_string(), &key, &event, &session::machine_id(&db)?, &peer)?);
        return Ok(out);
    }

    let event = crate::bridge::protocol::Event { name, data };
    if let Some((key, _)) = session::device_key(&db) {
        out.push(crate::bridge::outgoing(Kind::Event, uuid::Uuid::new_v4().to_string(), &key, &event)?);
    }
    if peers::remote_access_enabled(&db) {
        let own_id = session::machine_id(&db)?;
        for (peer, key) in peers::connected() {
            out.push(crate::bridge::outgoing_to_peer(Kind::Event, uuid::Uuid::new_v4().to_string(), &key, &event, &own_id, &peer)?);
        }
    }
    Ok(out)
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
