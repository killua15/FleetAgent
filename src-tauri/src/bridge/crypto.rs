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

/// This machine's long-lived X25519 identity, used to talk to the user's other
/// machines without a QR.
///
/// The phone still gets its key out of band (§7). Between machines of the same
/// account the public halves travel through `public.machines`, so the trust
/// root is the Supabase account — see the "Máquinas propias" decision in
/// `MOBILE_BRIDGE_TRD.md` §12. Like `SharedKey`, it does not implement `Debug`.
#[derive(Clone)]
pub struct MachineSecret([u8; KEY_BYTES]);

impl MachineSecret {
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

    pub fn to_base64(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(self.0)
    }

    /// The half that is published in `machines.public_key`.
    pub fn public_key(&self) -> String {
        let secret = x25519_dalek::StaticSecret::from(self.0);
        let public = x25519_dalek::PublicKey::from(&secret);
        base64::engine::general_purpose::STANDARD.encode(public.as_bytes())
    }
}

/// The symmetric key two machines share, derived from one's secret and the
/// other's public key. Both sides get the same bytes.
///
/// Byte-for-byte libsodium's `crypto_box_beforenm`: HSalsa20 over the raw
/// X25519 output with a zero input block. Never the raw X25519 output itself,
/// which is not uniformly random and must not be used as a key directly.
pub fn derive_peer_key(secret: &MachineSecret, peer_public_b64: &str) -> Result<SharedKey, String> {
    let raw = base64::engine::general_purpose::STANDARD.decode(peer_public_b64.trim()).map_err(|_| "clave pública ilegible".to_string())?;
    let public: [u8; KEY_BYTES] = raw.try_into().map_err(|_| format!("la clave pública debe tener {KEY_BYTES} bytes"))?;

    let shared = x25519_dalek::StaticSecret::from(secret.0).diffie_hellman(&x25519_dalek::PublicKey::from(public));
    // An all-zero result means the peer sent a low-order point: refusing it is
    // what libsodium does too, instead of handing back a key anyone can guess.
    if shared.as_bytes().iter().all(|b| *b == 0) {
        return Err("clave pública no válida".to_string());
    }

    let key = salsa20::hsalsa::<salsa20::cipher::consts::U10>(shared.as_bytes().into(), &[0u8; 16].into());
    Ok(SharedKey(key.into()))
}

/// What a person compares on both screens to know Supabase handed out the
/// right public key: the first 8 bytes of its SHA-256, as hex pairs.
pub fn fingerprint(public_b64: &str) -> String {
    use sha2::Digest;
    // Over the raw key, not its text: two encodings of one key are one key.
    let raw = base64::engine::general_purpose::STANDARD.decode(public_b64.trim()).unwrap_or_else(|_| public_b64.trim().as_bytes().to_vec());
    let digest = sha2::Sha256::digest(&raw);
    digest[..8].iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
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

    /// Both machines must land on the same key, or neither can read the other.
    #[test]
    fn two_machines_derive_the_same_key() {
        let a = MachineSecret::generate();
        let b = MachineSecret::generate();
        let ab = derive_peer_key(&a, &b.public_key()).unwrap();
        let ba = derive_peer_key(&b, &a.public_key()).unwrap();

        let (nonce, ciphertext) = seal(&ab, b"ls -la").unwrap();
        assert_eq!(open(&ba, &nonce, &ciphertext).unwrap(), b"ls -la");
    }

    /// A third machine of the same account cannot read a conversation between
    /// the other two.
    #[test]
    fn a_third_machine_cannot_read_the_pair() {
        let a = MachineSecret::generate();
        let b = MachineSecret::generate();
        let c = MachineSecret::generate();
        let (nonce, ciphertext) = seal(&derive_peer_key(&a, &b.public_key()).unwrap(), b"x").unwrap();
        assert!(open(&derive_peer_key(&c, &b.public_key()).unwrap(), &nonce, &ciphertext).is_err());
    }

    /// The all-zero point would give a key anyone can compute.
    #[test]
    fn a_low_order_public_key_is_refused() {
        let zero = base64::engine::general_purpose::STANDARD.encode([0u8; KEY_BYTES]);
        assert!(derive_peer_key(&MachineSecret::generate(), &zero).is_err());
    }

    #[test]
    fn the_fingerprint_is_stable_and_short() {
        let key = MachineSecret::generate().public_key();
        assert_eq!(fingerprint(&key), fingerprint(&key));
        assert_eq!(fingerprint(&key).len(), 8 * 3 - 1);
        assert_ne!(fingerprint(&key), fingerprint(&MachineSecret::generate().public_key()));
    }

    /// The derivation must match libsodium's `crypto_box_beforenm`, or a
    /// machine running another client could never talk to this one.
    /// `contract/interop.test.ts` checks the same fixture with libsodium-wrappers.
    #[test]
    fn the_peer_key_matches_the_committed_fixture() {
        let a = MachineSecret::from_base64("YWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWFhYWE=").unwrap();
        let b = MachineSecret::from_base64("YmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmJiYmI=").unwrap();
        let shared = derive_peer_key(&a, &b.public_key()).unwrap().to_base64();
        let path = fixture_path("peer-key-rust.json");

        if let Ok(raw) = std::fs::read_to_string(&path) {
            let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();
            assert_eq!(fixture["shared"].as_str().unwrap(), shared, "the X25519 derivation changed");
            assert_eq!(fixture["publicA"].as_str().unwrap(), a.public_key());
            return;
        }

        let fixture = serde_json::json!({
            "_comment": "Written once by Rust and committed. libsodium crypto_box_beforenm(publicB, secretA) must equal `shared`. Do not regenerate: see crypto.rs.",
            "secretA": a.to_base64(),
            "publicA": a.public_key(),
            "secretB": b.to_base64(),
            "publicB": b.public_key(),
            "shared": shared,
        });
        std::fs::write(&path, serde_json::to_string_pretty(&fixture).unwrap()).unwrap();
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
