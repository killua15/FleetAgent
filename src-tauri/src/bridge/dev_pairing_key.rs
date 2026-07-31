//! Guarda la clave del dispositivo emparejado en un archivo plano dentro de
//! `HANGAR_DATA_DIR`, en vez del Keychain — pero **solo** cuando esa variable
//! está puesta, es decir, nunca en un build instalado (`DATA_DIR_ENV` solo se
//! lee del entorno, un build de producción no lo tiene).
//!
//! Por qué hace falta esto y no solo `credential_vault`: una entrada de
//! Keychain escrita por un build de dev sin firmar no siempre es legible por
//! el siguiente build — cada recompilación puede verse como una app distinta
//! para macOS (ver el comentario de `credential_vault::service`). En la
//! práctica eso significaba volver a emparejar después de cada `cargo build`
//! durante una sesión de pruebas en vivo — friccion de desarrollo, no del
//! producto. En producción (build firmado, no se recompila en caliente) esto
//! no pasa nunca, así que ahí se sigue yendo al Keychain real, sin cambios.
//!
//! **Esto es deliberadamente menos seguro que el Keychain** — la clave queda
//! en texto plano en disco — y es aceptable únicamente porque está acotado a
//! una instancia de desarrollo aislada que un build real nunca alcanza.

use std::fs;
use std::path::{Path, PathBuf};

fn key_file(dir: &Path, credential_ref: &str) -> PathBuf {
    dir.join(format!("{credential_ref}.devkey"))
}

/// El directorio de la instancia de dev, si `HANGAR_DATA_DIR` está puesto.
/// `None` es la señal de "seguir yendo al Keychain real".
fn resolved_dir() -> Option<PathBuf> {
    let raw = std::env::var(crate::app_data::DATA_DIR_ENV).ok()?;
    Some(crate::app_data::expand_home(&raw, std::env::var("HOME").ok().as_deref()))
}

pub fn store(credential_ref: &str, secret: &str) -> Result<(), String> {
    match resolved_dir() {
        Some(dir) => store_in(&dir, credential_ref, secret),
        None => crate::credential_vault::store(credential_ref, secret),
    }
}

fn store_in(dir: &Path, credential_ref: &str, secret: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    fs::write(key_file(dir, credential_ref), secret).map_err(|e| e.to_string())
}

pub fn get(credential_ref: &str) -> Result<String, String> {
    match resolved_dir() {
        Some(dir) => get_in(&dir, credential_ref),
        None => crate::credential_vault::get(credential_ref),
    }
}

fn get_in(dir: &Path, credential_ref: &str) -> Result<String, String> {
    fs::read_to_string(key_file(dir, credential_ref)).map_err(|e| e.to_string())
}

pub fn has(credential_ref: &str) -> bool {
    match resolved_dir() {
        Some(dir) => key_file(&dir, credential_ref).exists(),
        None => crate::credential_vault::has(credential_ref),
    }
}

pub fn delete(credential_ref: &str) -> Result<(), String> {
    match resolved_dir() {
        Some(dir) => delete_in(&dir, credential_ref),
        None => crate::credential_vault::delete(credential_ref),
    }
}

fn delete_in(dir: &Path, credential_ref: &str) -> Result<(), String> {
    match fs::remove_file(key_file(dir, credential_ref)) {
        Ok(()) => Ok(()),
        // Borrar algo que ya no está no es un error — `credential_vault::delete`
        // trata `NoEntry` igual (ver su match de `keyring::Error::NoEntry`).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un directorio propio por test, no compartido: `resolved_dir()` lee el
    /// entorno real del proceso, que es global — pero estas pruebas ejercitan
    /// `*_in` directamente con un directorio pasado a mano, sin tocar el
    /// entorno, por la misma razón que `app_data::expand_home` se prueba
    /// suelta de `root`.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-dev-pairing-key-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn una_clave_guardada_se_puede_leer_de_vuelta() {
        let dir = temp_dir("roundtrip");
        store_in(&dir, "bridge-device-abc", "una-clave-en-base64").unwrap();
        assert_eq!(get_in(&dir, "bridge-device-abc").unwrap(), "una-clave-en-base64");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn leer_una_clave_que_nunca_se_guardo_es_un_error() {
        let dir = temp_dir("missing");
        assert!(get_in(&dir, "bridge-device-nunca-existio").is_err());
    }

    #[test]
    fn borrar_una_clave_inexistente_no_es_un_error() {
        let dir = temp_dir("delete-missing");
        assert!(delete_in(&dir, "bridge-device-nunca-existio").is_ok());
    }

    #[test]
    fn borrar_de_verdad_hace_que_una_lectura_posterior_falle() {
        let dir = temp_dir("delete-real");
        store_in(&dir, "bridge-device-xyz", "clave").unwrap();
        delete_in(&dir, "bridge-device-xyz").unwrap();
        assert!(get_in(&dir, "bridge-device-xyz").is_err());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dos_dispositivos_distintos_no_se_pisan() {
        let dir = temp_dir("two-devices");
        store_in(&dir, "bridge-device-uno", "clave-uno").unwrap();
        store_in(&dir, "bridge-device-dos", "clave-dos").unwrap();
        assert_eq!(get_in(&dir, "bridge-device-uno").unwrap(), "clave-uno");
        assert_eq!(get_in(&dir, "bridge-device-dos").unwrap(), "clave-dos");
        fs::remove_dir_all(&dir).ok();
    }
}
