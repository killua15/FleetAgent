//! Qué puede pedir el móvil, y con qué nivel.
//!
//! Es **un fichero de datos, no código** (`MOBILE_BRIDGE_TRD.md` §2): añadir una
//! función de Hangar a la app es añadir una línea a `allowlist.json`. Eso es lo
//! que hace que el escritorio no haya que tocarlo cada vez que crece el cliente.
//!
//! Y es lista blanca, no negra: un comando nuevo en Hangar **no** queda expuesto
//! por existir. Con 135 comandos, la política contraria significa que cualquier
//! cosa que se añada mañana es remota por accidente.

use crate::bridge::protocol::{ErrorCode, Failure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Los tres niveles, ordenados: quien puede lanzar puede dirigir, y quien dirige
/// puede mirar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// Estado, cuota, diffs, PRs. No escribe nada.
    Watch,
    /// Prompts, permisos, parar. Toca sesiones vivas.
    Steer,
    /// Abre worktrees y terminales. Crea trabajo.
    Launch,
}

const RAW: &str = include_str!("allowlist.json");

fn table() -> BTreeMap<String, Level> {
    let parsed: BTreeMap<String, serde_json::Value> = serde_json::from_str(RAW).expect("allowlist.json debe ser JSON válido");
    parsed
        .into_iter()
        // Las claves con `_` son comentarios: JSON no tiene, y el fichero lo lee
        // gente que necesita saber por qué está.
        .filter(|(name, _)| !name.starts_with('_'))
        .filter_map(|(name, value)| serde_json::from_value(value).ok().map(|level| (name, level)))
        .collect()
}

/// Los comandos que este dispositivo puede pedir — lo que va en el `hello`, para
/// que la app no ofrezca botones que van a fallar.
pub fn commands_for(level: Level) -> Vec<String> {
    table().into_iter().filter(|(_, needed)| *needed <= level).map(|(name, _)| name).collect()
}

/// La decisión, con los tres desenlaces separados porque son tres mensajes
/// distintos para quien los lee.
pub fn authorize(name: &str, level: Level) -> Result<(), Failure> {
    match table().get(name) {
        Some(needed) if *needed <= level => Ok(()),
        Some(needed) => Err(Failure::new(
            ErrorCode::Permission,
            format!("`{name}` necesita nivel {needed:?} y este dispositivo tiene {level:?}"),
        )),
        // Deliberadamente indistinguible de "no existe" hacia fuera no: el
        // código lo distingue (`NotAllowed` vs `UnknownCommand`) porque
        // "tu app es más nueva que este escritorio" y "eso no se expone" llevan
        // al usuario a sitios distintos. Aquí no sabemos cuál es; lo resuelve
        // quien conoce los comandos registrados.
        None => Err(Failure::new(ErrorCode::NotAllowed, format!("`{name}` no está expuesto al puente"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **El test que impide que renombrar un comando rompa el móvil en
    /// silencio.**
    ///
    /// Lee `lib.rs` y comprueba que cada nombre de la lista blanca está
    /// registrado de verdad. Sin esto, el fallo se descubre desde un teléfono,
    /// que es el peor sitio posible para descubrirlo.
    #[test]
    fn cada_comando_de_la_lista_existe_en_hangar() {
        let lib = include_str!("../lib.rs");
        let faltan: Vec<String> = table()
            .keys()
            .filter(|name| !lib.contains(&format!("::{name},")))
            .cloned()
            .collect();
        assert!(faltan.is_empty(), "expuestos pero no registrados en lib.rs: {faltan:?}");
    }

    /// Los niveles se ordenan, o `launch` no podría mirar.
    #[test]
    fn un_nivel_alto_incluye_a_los_bajos() {
        assert!(authorize("list_projects", Level::Watch).is_ok());
        assert!(authorize("list_projects", Level::Launch).is_ok());
        assert!(authorize("open_task_worktrees", Level::Launch).is_ok());
    }

    /// Y el que no llega, no pasa — con el código que dice por qué.
    #[test]
    fn mirar_no_puede_escribir() {
        let error = authorize("write_terminal", Level::Watch).unwrap_err();
        assert_eq!(error.code, ErrorCode::Permission);

        let error = authorize("open_task_worktrees", Level::Steer).unwrap_err();
        assert_eq!(error.code, ErrorCode::Permission);
    }

    /// Lista blanca: lo que no está, no pasa, aunque exista en Hangar.
    #[test]
    fn lo_que_no_esta_en_la_lista_no_pasa_aunque_exista() {
        // `delete_account` existe en Hangar y no está expuesto a propósito.
        let error = authorize("delete_account", Level::Launch).unwrap_err();
        assert_eq!(error.code, ErrorCode::NotAllowed);
    }

    /// Nada destructivo puede alcanzarse desde el móvil, por descuido o por
    /// tener el nivel más alto.
    #[test]
    fn ningun_comando_destructivo_esta_expuesto() {
        for peligroso in ["delete_account", "delete_project", "delete_session", "delete_task_source", "clear_project_git_identity"] {
            assert!(
                authorize(peligroso, Level::Launch).is_err(),
                "`{peligroso}` no debería alcanzarse desde un teléfono"
            );
        }
    }

    /// El `hello` lleva lo que ese dispositivo puede pedir; si mintiera, la app
    /// enseñaría botones que fallan.
    #[test]
    fn el_hello_solo_anuncia_lo_que_ese_nivel_alcanza() {
        let mirar = commands_for(Level::Watch);
        assert!(mirar.contains(&"list_accounts".to_string()));
        assert!(!mirar.contains(&"write_terminal".to_string()));

        assert!(commands_for(Level::Launch).len() > mirar.len());
    }
}
