//! Qué terminal está mirando el teléfono, y que se vea en el Mac.
//!
//! `MOBILE_BRIDGE_TRD.md` §14. La razón no es cosmética: **un agente dirigido
//! desde otro sitio no puede parecer igual que uno que nadie está tocando.** Sin
//! esa marca, alguien delante del Mac escribe en una terminal que el teléfono
//! está usando y ninguno de los dos entiende qué pasó.

use serde::Serialize;
use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Runtime};

/// La terminal seleccionada, si hay alguna.
///
/// **En memoria, no en la base**: es estado de una conexión viva, y al cerrarse
/// el puente deja de ser cierto. Persistirlo dejaría una marca en un panel para
/// un teléfono que ya no está.
static SELECTED: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub terminal_id: Option<String>,
    /// Quién la tiene, para que el panel pueda decirlo con nombre.
    pub device: Option<String>,
}

/// Lo que el teléfono llama al abrir una terminal.
///
/// No hay disputa que resolver: solo hay un dispositivo emparejado por Mac
/// (§12.3), así que seleccionar es **marcar**, no bloquear. Esa restricción
/// declarada de entrada es lo que evita tener que inventar turnos y caducidades.
///
/// **Seleccionar una terminal minimizada no la restaura.** Minimizar es una
/// decisión del escritorio y el móvil no la revoca — solo dice dónde está
/// mirando.
#[tauri::command]
pub fn bridge_select<R: Runtime>(app: AppHandle<R>, terminal_id: Option<String>) -> Result<(), String> {
    {
        let mut selected = SELECTED.lock().map_err(|e| e.to_string())?;
        *selected = terminal_id.filter(|id| !id.trim().is_empty());
    }
    announce(&app);
    Ok(())
}

/// La selección actual, para pintarla al montar la pantalla.
#[tauri::command]
pub fn bridge_selection<R: Runtime>(app: AppHandle<R>) -> Selection {
    current(&app)
}

/// Al desconectarse el teléfono la marca desaparece: un panel marcado para un
/// dispositivo ausente miente.
pub fn clear<R: Runtime>(app: &AppHandle<R>) {
    if let Ok(mut selected) = SELECTED.lock() {
        *selected = None;
    }
    announce(app);
}

fn current<R: Runtime>(app: &AppHandle<R>) -> Selection {
    use tauri::Manager;
    let terminal_id = SELECTED.lock().ok().and_then(|selected| selected.clone());
    let device = app
        .try_state::<crate::db::DbConnection>()
        .and_then(|db| crate::bridge::session::paired_device(&db))
        .map(|device| device.name);
    Selection { terminal_id, device }
}

fn announce<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.emit("bridge-selection", current(app));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sin selección no hay marca. Es el estado normal: el teléfono está
    /// emparejado casi siempre y mirando una terminal casi nunca.
    #[test]
    fn sin_seleccion_no_hay_terminal_marcada() {
        {
            let mut selected = SELECTED.lock().unwrap();
            *selected = None;
        }
        assert!(SELECTED.lock().unwrap().is_none());
    }

    /// Una cadena vacía es "ninguna", no una terminal llamada "". El móvil
    /// mandará eso al salir de la pantalla de detalle.
    #[test]
    fn una_seleccion_vacia_equivale_a_ninguna() {
        {
            let mut selected = SELECTED.lock().unwrap();
            *selected = Some("term-a".to_string());
        }
        {
            let mut selected = SELECTED.lock().unwrap();
            *selected = Some("   ".to_string()).filter(|id| !id.trim().is_empty());
        }
        assert!(SELECTED.lock().unwrap().is_none());
    }
}
