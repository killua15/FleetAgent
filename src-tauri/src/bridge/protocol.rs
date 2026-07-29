//! El sobre: lo único que Hangar y el móvil tienen que entender igual.
//!
//! Ver `MOBILE_BRIDGE_TRD.md` §4. Dos programas que se publican por separado
//! —el escritorio se actualiza solo, la app pasa por revisión— así que en
//! cualquier momento hay versiones que no coinciden. Esto es lo que hace que
//! eso sea aburrido.

use serde::{Deserialize, Serialize};

/// Se sube **solo** al romper el formato. Añadir un campo opcional o un comando
/// nuevo no es romperlo: un cliente viejo ignora lo que no conoce.
pub const PROTOCOL_VERSION: u16 = 1;

/// Lo que viaja por el canal. Todo lo demás va cifrado dentro de `payload`.
///
/// `name` **no** está aquí a propósito: saber que alguien pide
/// `list_account_spend` cada cinco minutos ya es información, y el relay no
/// tiene por qué tenerla.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Envelope {
    pub v: u16,
    /// Correlaciona una petición con su respuesta.
    pub id: String,
    pub kind: Kind,
    /// Base64. Nunca se repite con la misma clave — ver `crypto`.
    pub nonce: String,
    /// Base64 del texto cifrado. El claro es JSON.
    pub payload: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Hello,
    Invoke,
    Result,
    Error,
    Event,
}

/// Lo que el móvil pide.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Invoke {
    pub name: String,
    #[serde(default)]
    pub args: serde_json::Value,
}

/// Presentación. El móvil dice su versión; Hangar responde con la suya y con
/// **lo que ese dispositivo puede pedir**, para que la app no ofrezca botones
/// que van a fallar.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Hello {
    pub v: u16,
    pub device: String,
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub machine_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub name: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
}

/// Cada uno existe porque produce un mensaje distinto para quien lo lee.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    /// Las versiones no encajan. El mensaje lleva las dos, para que la app
    /// pueda decir "actualiza" en vez de fallar de forma rara.
    #[serde(rename = "E_PROTOCOL_VERSION")]
    ProtocolVersion,
    /// El comando existe en Hangar pero no está expuesto al puente.
    #[serde(rename = "E_NOT_ALLOWED")]
    NotAllowed,
    /// Está expuesto, pero pide más nivel del que tiene este dispositivo.
    #[serde(rename = "E_PERMISSION")]
    Permission,
    /// No existe. Separado de `NotAllowed` porque casi siempre significa "tu
    /// app es más nueva que este escritorio", que es otra conversación.
    #[serde(rename = "E_UNKNOWN_COMMAND")]
    UnknownCommand,
    /// Clave equivocada o mensaje manipulado. Nunca se registra el contenido.
    #[serde(rename = "E_DECRYPT")]
    Decrypt,
    /// El comando corrió y devolvió error. `message` es el suyo, tal cual.
    #[serde(rename = "E_COMMAND_FAILED")]
    CommandFailed,
    /// La respuesta no cabe en el canal.
    #[serde(rename = "E_TOO_LARGE")]
    TooLarge,
    /// Este dispositivo fue sustituido por otro emparejamiento.
    #[serde(rename = "E_DEVICE_REPLACED")]
    DeviceReplaced,
}

impl Failure {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    /// Con las dos versiones dentro: un cliente que solo recibe "no encaja" no
    /// puede decidir si el viejo es él o el escritorio.
    pub fn version_mismatch(theirs: u16) -> Self {
        Self::new(
            ErrorCode::ProtocolVersion,
            format!("this Hangar speaks protocol v{PROTOCOL_VERSION}, the client speaks v{theirs}"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Los nombres de los códigos viajan por el cable y los lee Dart. Cambiar
    /// uno rompe la app sin que Rust se entere, así que se fijan aquí.
    #[test]
    fn los_codigos_de_error_tienen_los_nombres_del_contrato() {
        let json = serde_json::to_string(&ErrorCode::UnknownCommand).unwrap();
        assert_eq!(json, "\"E_UNKNOWN_COMMAND\"");
        assert_eq!(serde_json::to_string(&ErrorCode::Decrypt).unwrap(), "\"E_DECRYPT\"");
        assert_eq!(serde_json::to_string(&ErrorCode::DeviceReplaced).unwrap(), "\"E_DEVICE_REPLACED\"");
    }

    /// Un cliente más nuevo mandará campos que este escritorio no conoce. Tiene
    /// que ignorarlos, no rechazar el mensaje: es la mitad de "solo aditivo".
    #[test]
    fn un_mensaje_con_campos_de_mas_se_lee_igual() {
        let raw = r#"{"v":1,"id":"01J","kind":"invoke","nonce":"AA","payload":"BB","futuro":42}"#;
        let envelope: Envelope = serde_json::from_str(raw).expect("los campos desconocidos se ignoran");
        assert_eq!(envelope.kind, Kind::Invoke);
    }

    /// Y un `invoke` sin `args` es válido: la mitad de los comandos no llevan.
    #[test]
    fn un_invoke_sin_argumentos_es_valido() {
        let invoke: Invoke = serde_json::from_str(r#"{"name":"list_projects"}"#).unwrap();
        assert_eq!(invoke.args, serde_json::Value::Null);
    }

    /// El error de versión lleva las dos, o el usuario no sabe qué actualizar.
    #[test]
    fn el_error_de_version_nombra_las_dos_partes() {
        let failure = Failure::version_mismatch(9);
        assert!(failure.message.contains("v1"), "{}", failure.message);
        assert!(failure.message.contains("v9"), "{}", failure.message);
    }
}
