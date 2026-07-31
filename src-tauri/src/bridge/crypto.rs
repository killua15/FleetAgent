//! Cifrado del puente: `crypto_secretbox` de libsodium (XSalsa20-Poly1305).
//!
//! **Simétrico y no asimétrico** porque el emparejamiento entrega la clave en el
//! QR (`MOBILE_BRIDGE_TRD.md` §7) — un canal que se ve con los ojos y no
//! atraviesa ningún servidor. No hay terceros que necesiten cifrar sin poder
//! descifrar, que es lo único que justificaría claves asimétricas aquí.
//!
//! La primitiva se eligió por interoperar: la misma que usan `sodium` en Dart
//! (el sucesor de `sodium_libs`, descontinuado desde que los build hooks de
//! Dart pueden empaquetar el binario nativo directamente) y
//! `libsodium-wrappers` en Node, sin arrastrar una dependencia de C.

use base64::Engine;
use crypto_secretbox::aead::{Aead, KeyInit};
use crypto_secretbox::{Nonce, XSalsa20Poly1305};
use rand::RngCore;

pub const KEY_BYTES: usize = 32;
pub const NONCE_BYTES: usize = 24;

/// La clave compartida de un emparejamiento. Vive en el llavero, como todo
/// secreto de este proyecto.
#[derive(Clone)]
pub struct SharedKey([u8; KEY_BYTES]);

impl SharedKey {
    pub fn generate() -> Self {
        let mut key = [0u8; KEY_BYTES];
        rand::thread_rng().fill_bytes(&mut key);
        Self(key)
    }

    pub fn from_base64(encoded: &str) -> Result<Self, String> {
        let raw = base64::engine::general_purpose::STANDARD.decode(encoded.trim()).map_err(|e| format!("clave ilegible: {e}"))?;
        let key: [u8; KEY_BYTES] = raw.try_into().map_err(|_| format!("la clave debe tener {KEY_BYTES} bytes"))?;
        Ok(Self(key))
    }

    /// Para el QR y para guardarla. No implementa `Debug` a propósito: una clave
    /// que se puede imprimir acaba impresa.
    pub fn to_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.0)
    }
}

/// Cifra, devolviendo `(nonce, texto cifrado)` en base64.
///
/// El nonce es **aleatorio en cada mensaje**. Nunca un contador ni derivado del
/// reloj: repetir un nonce con la misma clave rompe XSalsa20 por completo, y un
/// contador se repite en cuanto dos procesos comparten clave o uno reinicia.
pub fn seal(key: &SharedKey, plaintext: &[u8]) -> Result<(String, String), String> {
    let cipher = XSalsa20Poly1305::new_from_slice(&key.0).map_err(|e| e.to_string())?;

    let mut nonce_bytes = [0u8; NONCE_BYTES];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher.encrypt(nonce, plaintext).map_err(|_| "no se pudo cifrar".to_string())?;
    let encode = base64::engine::general_purpose::STANDARD;
    Ok((encode.encode(nonce_bytes), encode.encode(ciphertext)))
}

/// Descifra. Un fallo aquí es clave equivocada o mensaje manipulado, y las dos
/// cosas se tratan igual: se descarta.
///
/// **El error nunca lleva el contenido.** Un log con payloads es una filtración
/// con retraso.
pub fn open(key: &SharedKey, nonce_b64: &str, ciphertext_b64: &str) -> Result<Vec<u8>, String> {
    let decode = base64::engine::general_purpose::STANDARD;
    let nonce_bytes = decode.decode(nonce_b64).map_err(|_| "nonce ilegible".to_string())?;
    if nonce_bytes.len() != NONCE_BYTES {
        return Err(format!("el nonce debe tener {NONCE_BYTES} bytes"));
    }
    let ciphertext = decode.decode(ciphertext_b64).map_err(|_| "texto cifrado ilegible".to_string())?;

    let cipher = XSalsa20Poly1305::new_from_slice(&key.0).map_err(|e| e.to_string())?;
    cipher
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| "no se pudo descifrar: clave equivocada o mensaje manipulado".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lo_cifrado_se_recupera_igual() {
        let key = SharedKey::generate();
        let (nonce, ciphertext) = seal(&key, b"hola").unwrap();
        assert_eq!(open(&key, &nonce, &ciphertext).unwrap(), b"hola");
    }

    /// La razón de existir del cifrado: Supabase transporta esto y no puede
    /// abrirlo.
    #[test]
    fn otra_clave_no_abre_el_mensaje() {
        let (nonce, ciphertext) = seal(&SharedKey::generate(), b"secreto").unwrap();
        let error = open(&SharedKey::generate(), &nonce, &ciphertext).unwrap_err();
        assert!(!error.contains("secreto"), "el error no puede filtrar el contenido: {error}");
    }

    /// Poly1305 autentica: cambiar un byte tiene que fallar, no devolver basura.
    #[test]
    fn un_mensaje_manipulado_no_se_acepta() {
        let key = SharedKey::generate();
        let (nonce, ciphertext) = seal(&key, b"transferir 10").unwrap();

        let mut raw = base64::engine::general_purpose::STANDARD.decode(&ciphertext).unwrap();
        raw[0] ^= 0x01;
        let tampered = base64::engine::general_purpose::STANDARD.encode(raw);

        assert!(open(&key, &nonce, &tampered).is_err());
    }

    /// El fallo criptográfico clásico. Mil mensajes, mil nonces distintos.
    #[test]
    fn los_nonces_no_se_repiten() {
        let key = SharedKey::generate();
        let nonces: std::collections::BTreeSet<String> = (0..1000).map(|_| seal(&key, b"x").unwrap().0).collect();
        assert_eq!(nonces.len(), 1000, "un nonce repetido con la misma clave rompe el cifrado entero");
    }

    /// El mismo texto cifrado dos veces tiene que verse distinto, o el relay
    /// aprende cuándo se repite una orden sin descifrar nada.
    #[test]
    fn el_mismo_texto_produce_cifrados_distintos() {
        let key = SharedKey::generate();
        let (_, a) = seal(&key, b"list_projects").unwrap();
        let (_, b) = seal(&key, b"list_projects").unwrap();
        assert_ne!(a, b);
    }

    /// Ruta del blob que produce Rust y que el resto tiene que poder abrir.
    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../contract/fixtures").join(name)
    }

    /// El blob de bytes fijos que Node y Dart tienen que abrir.
    ///
    /// Que Rust se entienda consigo mismo no dice **nada** sobre Dart ni sobre
    /// Node: la primitiva se eligió justamente por interoperar, y eso hay que
    /// demostrarlo cruzando el lenguaje. Este fichero lo lee
    /// `contract/interop.test.ts`, y lo leerá el repo de Flutter.
    ///
    /// **Se escribe una sola vez y luego se comprueba.** Antes se regeneraba en
    /// cada ejecución con un nonce nuevo, y un golden que se reescribe a sí mismo
    /// no puede fallar nunca: si un cambio rompiera la compatibilidad, el fichero
    /// cambiaría con él y el otro lado abriría el blob nuevo sin enterarse. Peor
    /// para lo que viene: el repo de Flutter es otro repo y no puede validar
    /// contra un fichero que se mueve cada vez que alguien corre los tests aquí.
    #[test]
    fn el_blob_de_interoperabilidad_es_estable() {
        let key = SharedKey::from_base64("MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=").expect("clave fija, no aleatoria: el fichero se commitea");
        let plaintext = r#"{"name":"list_projects","args":{}}"#;
        let path = fixture_path("crypto-rust.json");

        if let Ok(raw) = std::fs::read_to_string(&path) {
            // Lo normal: existe y hay que poder abrirlo. Esto es lo que detecta
            // que alguien cambió la primitiva, el orden de los argumentos o la
            // variante de base64.
            let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();
            let opened = open(&key, fixture["nonce"].as_str().unwrap(), fixture["ciphertext"].as_str().unwrap())
                .expect("Rust tiene que abrir el blob que Rust dejó commiteado; si esto falla, el móvil ya no habla con el escritorio");
            assert_eq!(String::from_utf8(opened).unwrap(), plaintext);
            return;
        }

        let (nonce, ciphertext) = seal(&key, plaintext.as_bytes()).unwrap();
        let fixture = serde_json::json!({
            "_comment": "Producido por Rust una sola vez y commiteado. Node y Dart tienen que abrirlo con la misma clave. No regenerar: ver crypto.rs.",
            "key": key.to_base64(),
            "nonce": nonce,
            "ciphertext": ciphertext,
            "plaintext": plaintext,
        });
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&fixture).unwrap()).unwrap();
    }

    /// Y al revés: abre lo que produjo Node.
    ///
    /// Si el fichero no está es un fallo, no un test que se salta. Está
    /// commiteado, así que ausente solo puede significar que alguien lo borró — y
    /// saltar en silencio dejaría la mitad del contrato sin comprobar
    /// exactamente cuando importa.
    #[test]
    fn abre_lo_que_cifro_node() {
        let path = fixture_path("crypto-node.json");
        let raw = std::fs::read_to_string(&path).expect("falta contract/fixtures/crypto-node.json, que va commiteado: sin él no hay prueba de interoperabilidad");
        let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();

        let key = SharedKey::from_base64(fixture["key"].as_str().unwrap()).unwrap();
        let plaintext = open(&key, fixture["nonce"].as_str().unwrap(), fixture["ciphertext"].as_str().unwrap())
            .expect("lo que cifra Node tiene que abrirlo Rust, o el móvil no hablará con el escritorio");

        assert_eq!(String::from_utf8(plaintext).unwrap(), fixture["plaintext"].as_str().unwrap());
    }

    /// Y lo mismo con Dart: sin esto, el cruce de tres lenguajes que pide el
    /// arranque móvil ("si Dart abre el de Rust y Rust abre el de Dart,
    /// hablaréis") solo estaría comprobado en dos direcciones de tres.
    #[test]
    fn abre_lo_que_cifro_dart() {
        let path = fixture_path("crypto-dart.json");
        let raw = std::fs::read_to_string(&path).expect("falta contract/fixtures/crypto-dart.json, que va commiteado: sin él no hay prueba de interoperabilidad con Flutter");
        let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();

        let key = SharedKey::from_base64(fixture["key"].as_str().unwrap()).unwrap();
        let plaintext = open(&key, fixture["nonce"].as_str().unwrap(), fixture["ciphertext"].as_str().unwrap())
            .expect("lo que cifra Dart tiene que abrirlo Rust, o el móvil no hablará con el escritorio");

        assert_eq!(String::from_utf8(plaintext).unwrap(), fixture["plaintext"].as_str().unwrap());
    }

    /// La clave va en el QR como texto: tiene que sobrevivir el viaje.
    #[test]
    fn la_clave_sobrevive_a_base64() {
        let key = SharedKey::generate();
        let recovered = SharedKey::from_base64(&key.to_base64()).unwrap();
        let (nonce, ciphertext) = seal(&key, b"ida").unwrap();
        assert_eq!(open(&recovered, &nonce, &ciphertext).unwrap(), b"ida");

        assert!(SharedKey::from_base64("demasiado-corta").is_err());
    }
}
