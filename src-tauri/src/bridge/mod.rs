//! El puente móvil: qué se habla (`protocol`), cómo se cifra (`crypto`) y qué
//! se permite (`allowlist`).
//!
//! Fase 0 del plan en `docs/mobile/`. Aquí no hay red ni Supabase: eso es la
//! fase 1. Este módulo define el idioma y se prueba solo, que es justo lo que
//! permite equivocarse barato.

pub mod allowlist;
pub mod crypto;
pub mod dispatch;
pub mod protocol;
pub mod session;

use allowlist::Level;
use protocol::{Envelope, ErrorCode, Failure, Invoke, Kind, PROTOCOL_VERSION};

/// Lo que hay que hacer con un mensaje entrante, una vez descifrado y validado.
#[derive(Debug, PartialEq)]
pub enum Incoming {
    /// Ejecutar este comando. Quien lo reciba ya no tiene que comprobar nada:
    /// versión, cifrado y permiso están resueltos.
    Run { id: String, invoke: Invoke },
    /// Responder a la presentación con nuestra versión y sus comandos.
    Greet { id: String, level: Level },
    /// Devolver este error tal cual.
    Reject { id: String, failure: Failure },
}

/// Descifra, valida y autoriza. **Todo pasa por aquí antes de tocar nada.**
///
/// El orden importa: primero la versión (un cliente incompatible merece un
/// mensaje claro, no un fallo de descifrado), luego el cifrado, y solo después
/// el permiso — comprobar permisos sobre algo que no se ha podido autenticar
/// sería opinar sobre un mensaje de cualquiera.
pub fn handle_incoming(envelope: &Envelope, key: &crypto::SharedKey, level: Level) -> Incoming {
    let id = envelope.id.clone();

    if envelope.v != PROTOCOL_VERSION {
        return Incoming::Reject { id, failure: Failure::version_mismatch(envelope.v) };
    }

    let plaintext = match crypto::open(key, &envelope.nonce, &envelope.payload) {
        Ok(bytes) => bytes,
        // Sin el contenido: un log con payloads es una filtración con retraso.
        Err(_) => {
            return Incoming::Reject {
                id,
                failure: Failure::new(ErrorCode::Decrypt, "no se pudo descifrar el mensaje"),
            }
        }
    };

    match envelope.kind {
        Kind::Hello => Incoming::Greet { id, level },
        Kind::Invoke => match serde_json::from_slice::<Invoke>(&plaintext) {
            Ok(invoke) => match allowlist::authorize(&invoke.name, level) {
                Ok(()) => Incoming::Run { id, invoke },
                Err(failure) => Incoming::Reject { id, failure },
            },
            Err(e) => Incoming::Reject {
                id,
                failure: Failure::new(ErrorCode::CommandFailed, format!("payload ilegible: {e}")),
            },
        },
        // Un cliente no manda resultados, errores ni eventos: eso viaja del Mac
        // hacia fuera. Recibirlo significa que algo está mal montado.
        Kind::Result | Kind::Error | Kind::Event => Incoming::Reject {
            id,
            failure: Failure::new(ErrorCode::NotAllowed, "un cliente no envía mensajes de ese tipo"),
        },
    }
}

/// Empaqueta algo para el otro lado. Un id nuevo para eventos; el de la
/// petición para respuestas.
pub fn outgoing(kind: Kind, id: String, key: &crypto::SharedKey, body: &impl serde::Serialize) -> Result<Envelope, String> {
    let plaintext = serde_json::to_vec(body).map_err(|e| e.to_string())?;
    let (nonce, payload) = crypto::seal(key, &plaintext)?;
    Ok(Envelope { v: PROTOCOL_VERSION, id, kind, nonce, payload })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealed(key: &crypto::SharedKey, kind: Kind, body: &impl serde::Serialize) -> Envelope {
        outgoing(kind, "01J".to_string(), key, body).unwrap()
    }

    #[test]
    fn un_invoke_permitido_llega_listo_para_ejecutar() {
        let key = crypto::SharedKey::generate();
        let envelope = sealed(&key, Kind::Invoke, &Invoke { name: "list_projects".into(), args: serde_json::json!({}) });

        match handle_incoming(&envelope, &key, Level::Watch) {
            Incoming::Run { invoke, .. } => assert_eq!(invoke.name, "list_projects"),
            other => panic!("esperaba ejecutar, salió {other:?}"),
        }
    }

    #[test]
    fn sin_nivel_suficiente_no_se_ejecuta() {
        let key = crypto::SharedKey::generate();
        let envelope = sealed(&key, Kind::Invoke, &Invoke { name: "write_terminal".into(), args: serde_json::json!({}) });

        match handle_incoming(&envelope, &key, Level::Watch) {
            Incoming::Reject { failure, .. } => assert_eq!(failure.code, ErrorCode::Permission),
            other => panic!("esperaba rechazo, salió {other:?}"),
        }
    }

    /// Con la clave equivocada no se llega ni a mirar qué pedía.
    #[test]
    fn un_mensaje_de_otra_clave_se_descarta_sin_leerlo() {
        let key = crypto::SharedKey::generate();
        let envelope = sealed(&key, Kind::Invoke, &Invoke { name: "list_projects".into(), args: serde_json::json!({}) });

        match handle_incoming(&envelope, &crypto::SharedKey::generate(), Level::Launch) {
            Incoming::Reject { failure, .. } => {
                assert_eq!(failure.code, ErrorCode::Decrypt);
                assert!(!failure.message.contains("list_projects"), "el error no puede decir qué pedía");
            }
            other => panic!("esperaba rechazo, salió {other:?}"),
        }
    }

    /// La versión se comprueba **antes** que el cifrado: un cliente viejo con
    /// otro formato tiene que oír "actualiza", no "no se pudo descifrar".
    #[test]
    fn la_version_se_comprueba_antes_que_la_clave() {
        let key = crypto::SharedKey::generate();
        let mut envelope = sealed(&key, Kind::Invoke, &Invoke { name: "list_projects".into(), args: serde_json::json!({}) });
        envelope.v = 99;

        match handle_incoming(&envelope, &crypto::SharedKey::generate(), Level::Watch) {
            Incoming::Reject { failure, .. } => assert_eq!(failure.code, ErrorCode::ProtocolVersion),
            other => panic!("esperaba rechazo por versión, salió {other:?}"),
        }
    }

    /// Un cliente que mande un evento está mal montado, o es alguien probando.
    #[test]
    fn un_cliente_no_puede_inyectar_eventos() {
        let key = crypto::SharedKey::generate();
        let envelope = sealed(&key, Kind::Event, &protocol::Event { name: "terminal-state".into(), data: serde_json::json!({}) });

        match handle_incoming(&envelope, &key, Level::Launch) {
            Incoming::Reject { failure, .. } => assert_eq!(failure.code, ErrorCode::NotAllowed),
            other => panic!("esperaba rechazo, salió {other:?}"),
        }
    }
}
