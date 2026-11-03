//! How what the Factory builds looks. The creator decides it in the brain's
//! «Diseño» section — with references and, if they like, one of a few
//! directions the AI proposes — the line builds it first as a system (tokens,
//! base components, a living style guide at `/design`), the creator approves
//! it by seeing it, and every screen after is designed and reviewed against it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use super::agent::{AgentCall, CliRunner, Outcome, Runner};
use super::brain::Rule;
use crate::db::DbConnection;

/// Where a project's design assets live: app data, not the repo — they're
/// the creator's, like the brain.
pub fn design_dir(root: &Path, project_id: &str) -> PathBuf {
    root.join("factory").join(project_id).join("design")
}

pub fn refs_dir(root: &Path, project_id: &str) -> PathBuf {
    design_dir(root, project_id).join("refs")
}

/// The style guide's screenshots the creator approved: the project's visual
/// reference from then on.
pub fn approved_dir(root: &Path, project_id: &str) -> PathBuf {
    design_dir(root, project_id).join("aprobado")
}

/// Whether the brain says the project has an interface to design.
pub fn has_ui(rules: &[Rule]) -> bool {
    let dis: Vec<&Rule> = rules.iter().filter(|r| r.section == "dis").collect();
    !dis.is_empty() && !dis.iter().any(|r| r.text.trim().eq_ignore_ascii_case("Tipo de app: sin interfaz"))
}

/// Where in a worktree the agents find the design assets.
pub const IN_WORKTREE: &str = ".hangar/design";

/// Copies the references and the approved screenshots into the worktree, so
/// the designer, the builder and the UI reviewer can open them. `.hangar` is
/// excluded from git, so none of it is committed. Returns what's there.
pub fn stage_assets(root: &Path, project_id: &str, worktree: &Path) -> Vec<String> {
    let mut found = Vec::new();
    for (from, name) in [(refs_dir(root, project_id), "refs"), (approved_dir(root, project_id), "aprobado")] {
        let Ok(entries) = std::fs::read_dir(&from) else { continue };
        let to = worktree.join(IN_WORKTREE).join(name);
        let _ = std::fs::create_dir_all(&to);
        for e in entries.flatten() {
            let path = e.path();
            if path.is_file() {
                let file = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                if std::fs::copy(&path, to.join(&file)).is_ok() {
                    found.push(format!("{IN_WORKTREE}/{name}/{file}"));
                }
            }
        }
    }
    found.sort();
    found
}

/// What the design agents are told about the assets.
pub fn assets_prompt(files: &[String]) -> String {
    if files.is_empty() {
        return String::new();
    }
    format!(
        "\n=== DISEÑO: REFERENCIAS Y GUÍA APROBADA ===\nÁbrelas con Read antes de nada (son imágenes): `refs/` son referencias del creador; `aprobado/` son capturas de la guía /design que el creador aprobó — la referencia visual del proyecto.\n{}\n",
        files.iter().map(|f| format!("- {f}")).collect::<Vec<_>>().join("\n")
    )
}

// ---------------------------------------------------------------- screenshots

/// A browser that can take a headless screenshot.
pub fn chrome() -> Option<PathBuf> {
    let candidates = [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
    ];
    if let Some(p) = candidates.iter().map(PathBuf::from).find(|p| p.is_file()) {
        return Some(p);
    }
    ["google-chrome", "chromium", "chromium-browser"].iter().find(|b| super::env::has_executable(b)).map(PathBuf::from)
}

/// The sizes and themes the style guide is looked at in.
pub const SHOTS: [(&str, u32, u32, bool); 4] = [("móvil · claro", 390, 844, false), ("escritorio · claro", 1280, 860, false), ("móvil · oscuro", 390, 844, true), ("escritorio · oscuro", 1280, 860, true)];

fn shot_file(label: &str) -> String {
    label.replace(" · ", "-").replace('ó', "o").replace(' ', "-") + ".png"
}

/// Screenshots of `url` in every size and theme of `SHOTS`, into `dir`.
/// Without a browser, or with the app not answering, it says why.
pub fn capture(project_id: &str, url: &str, dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let browser = chrome().ok_or("no hay Chrome, Chromium, Edge ni Brave para hacer las capturas")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (label, w, h, dark) in SHOTS {
        let path = dir.join(shot_file(label));
        let mut cmd = std::process::Command::new(&browser);
        cmd.args(["--headless=new", "--disable-gpu", "--hide-scrollbars", "--no-first-run", "--no-default-browser-check", "--virtual-time-budget=6000"]);
        cmd.arg(format!("--window-size={w},{h}"));
        cmd.arg(format!("--screenshot={}", path.display()));
        // Always say which theme: headless Chrome otherwise follows the
        // system's, and a «light» shot comes out dark on a dark Mac.
        // (Blink: preferredColorScheme 0 = dark, 1 = light.)
        if dark {
            cmd.arg("--force-dark-mode");
            cmd.arg("--blink-settings=preferredColorScheme=0");
        } else {
            cmd.arg("--blink-settings=preferredColorScheme=1");
        }
        cmd.arg(url);
        let _ = super::procs::run(project_id, "design-shots", cmd, Duration::from_secs(60));
        if path.is_file() {
            out.push((label.to_string(), path));
        }
    }
    if out.is_empty() {
        return Err(format!("no salió ninguna captura de {url}"));
    }
    Ok(out)
}

fn data_url(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => return None,
    };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Shot {
    pub label: String,
    pub data_url: String,
}

/// The screenshots a run took for the creator to approve.
#[tauri::command]
pub async fn factory_design_shots<R: Runtime>(app: AppHandle<R>, run_id: String) -> Result<Vec<Shot>, String> {
    let run = crate::factory::with_db(app, move |conn| super::line::load_run(conn, &run_id)).await?;
    crate::blocking::run_blocking(move || run.state.design_shots.iter().filter_map(|(label, path)| data_url(Path::new(path)).map(|data_url| Shot { label: label.clone(), data_url })).collect()).await
}

// ---------------------------------------------------------------- references

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub name: String,
    pub data_url: Option<String>,
}

fn list_refs(dir: &Path) -> Vec<Reference> {
    let mut out: Vec<Reference> = std::fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).filter(|p| p.is_file()).map(|p| Reference { name: p.file_name().unwrap_or_default().to_string_lossy().to_string(), data_url: data_url(&p) }).collect())
        .unwrap_or_default();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[tauri::command]
pub async fn factory_design_refs<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<Vec<Reference>, String> {
    let root = crate::app_data::root(&app)?;
    crate::blocking::run_blocking(move || list_refs(&refs_dir(&root, &project_id))).await
}

/// «Añadir imágenes»: copies the chosen images into the project's references.
#[tauri::command]
pub async fn factory_design_add_refs<R: Runtime>(app: AppHandle<R>, project_id: String, paths: Vec<String>) -> Result<Vec<Reference>, String> {
    let root = crate::app_data::root(&app)?;
    crate::blocking::run_blocking(move || {
        let dir = refs_dir(&root, &project_id);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for p in &paths {
            let from = Path::new(p);
            if data_url(from).is_none() {
                return Err(format!("{} no es una imagen (png, jpg, gif, webp o svg)", from.display()));
            }
            let name = from.file_name().ok_or("ruta sin nombre de archivo")?;
            std::fs::copy(from, dir.join(name)).map_err(|e| format!("no se pudo copiar {}: {e}", from.display()))?;
        }
        Ok(list_refs(&dir))
    })
    .await?
}

#[tauri::command]
pub async fn factory_design_remove_ref<R: Runtime>(app: AppHandle<R>, project_id: String, name: String) -> Result<Vec<Reference>, String> {
    let root = crate::app_data::root(&app)?;
    crate::blocking::run_blocking(move || {
        let dir = refs_dir(&root, &project_id);
        if name.contains('/') || name.contains("..") {
            return Err("nombre no válido".into());
        }
        let _ = std::fs::remove_file(dir.join(&name));
        Ok(list_refs(&dir))
    })
    .await?
}

// ---------------------------------------------------------------- directions

/// One visual direction the AI proposes, with a page to see it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Direction {
    pub name: String,
    pub mood: String,
    pub colors: Vec<String>,
    pub font: String,
    pub shape: String,
    pub library: String,
    pub dark: bool,
    pub preview_html: String,
}

pub fn directions_schema() -> Value {
    let s = json!({"type":"string"});
    json!({
        "type": "object",
        "required": ["directions"],
        "properties": { "directions": { "type": "array", "items": { "type": "object",
            "required": ["name", "mood", "colors", "font", "shape", "library", "dark", "preview_html"],
            "properties": { "name": s, "mood": s, "colors": {"type":"array","items":{"type":"string"}}, "font": s, "shape": s, "library": s, "dark": {"type":"boolean"}, "preview_html": s } } } }
    })
}

pub fn directions_prompt(project: &str, product: &str, refs: &[String]) -> String {
    format!(
        "Eres diseñador de producto. Propón 3 direcciones visuales CLARAMENTE distintas para «{project}». Nada genérico: cada una con personalidad propia y coherente con el producto.\n\n\
         === EL PRODUCTO ===\n{product}\n\n{}\
         Para cada dirección:\n\
         - name: un nombre corto; mood: el estilo en una frase concreta;\n\
         - colors: «nombre #hex», 5–7 (primario, acento, fondo, superficie, texto, borde, error), con contraste WCAG AA entre texto y fondo;\n\
         - font: familia y escala (de Google Fonts o del sistema); shape: radios, espaciado y sombras; library: la librería de componentes que mejor la sirve;\n\
         - dark: si la propones con modo oscuro;\n\
         - preview_html: una página HTML completa y autocontenida (CSS en <style>, SIN JavaScript, sin imágenes externas; Google Fonts permitido) que muestre la pantalla principal del producto con datos de ejemplo realistas, usando exactamente esos colores, tipografía y forma. Que se vea terminada.\n",
        if refs.is_empty() { String::new() } else { format!("=== REFERENCIAS DEL CREADOR (ábrelas con Read) ===\n{}\n\n", refs.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n")) }
    )
}

pub fn read_directions(answer: &Value) -> Vec<Direction> {
    answer
        .get("directions")
        .and_then(Value::as_array)
        .map(|ds| ds.iter().filter_map(|d| serde_json::from_value::<Direction>(rename_preview(d.clone())).ok()).take(3).collect())
        .unwrap_or_default()
}

fn rename_preview(mut d: Value) -> Value {
    if let Some(html) = d.get("preview_html").cloned() {
        d["previewHtml"] = html;
    }
    d
}

/// «✦ Proponer estilos»: the spec writer reads the product and the creator's
/// references and proposes three directions, each with a page to look at.
#[tauri::command]
pub async fn factory_design_propose<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<Vec<Direction>, String> {
    crate::blocking::run_blocking(move || {
        let root = crate::app_data::root(&app)?;
        let db = app.state::<DbConnection>();
        let (project, product, account, model, accounts) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let project = super::store::get(&conn, &project_id)?;
            let items = super::decide::load_draft(&conn, &project_id)?.map(|(i, _)| i).unwrap_or_default();
            let mut product: Vec<String> = super::decide::render_rules(&items).into_iter().filter(|r| r.section == "prd" || r.section == "dis").map(|r| format!("- {}", r.text)).collect();
            if product.is_empty() {
                if let Some(b) = super::brain::load(&conn, &project_id, None)? {
                    product = b.rules.iter().filter(|r| r.section == "prd" || r.section == "dis").map(|r| format!("- {}", r.text)).collect();
                }
            }
            let (account, model) = super::planner::spec_agent(&conn, &project_id)?;
            let accounts = super::agent::accounts_for(&conn, std::slice::from_ref(&account))?;
            (project, product.join("\n"), account, model, accounts)
        };
        let runner = CliRunner::from_accounts(accounts, Some(root.clone()))?;
        // The references, where the agent (in the project folder) can open them.
        let refs = stage_assets(&root, &project_id, Path::new(&project.project_path));
        super::procs::clear_job(&project_id, "design");
        let call = AgentCall {
            project: project_id.clone(),
            key: "design".into(),
            account_id: account,
            model,
            cwd: project.project_path.clone().into(),
            prompt: directions_prompt(&project.project_name, &product, &refs),
            schema: directions_schema(),
            resume: None,
            mcp_config: None,
            write: false,
            timeout: Duration::from_secs(20 * 60),
        };
        let result = runner.run(&call, &mut |_| {});
        match result.outcome {
            Outcome::Ok => {
                let ds = read_directions(result.json.as_ref().ok_or("la IA no devolvió direcciones")?);
                if ds.is_empty() {
                    return Err("la IA no devolvió ninguna dirección utilizable".into());
                }
                Ok(ds)
            }
            Outcome::Limit { resume_at } => Err(format!("la cuenta del spec writer llegó a su límite; se renueva a las {}", resume_at.with_timezone(&chrono::Local).format("%H:%M"))),
            Outcome::NoCredits => Err("la cuenta del spec writer no tiene créditos".into()),
            Outcome::Stopped => Err("cancelado".into()),
            Outcome::Failed(why) => Err(format!("la IA falló: {why}")),
        }
    })
    .await?
}

#[tauri::command]
pub fn factory_design_cancel(project_id: String) -> bool {
    super::procs::stop_job(&project_id, "design") > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(section: &str, text: &str) -> Rule {
        Rule { id: "X-01".into(), section: section.into(), level: "must".into(), text: text.into(), check: None }
    }

    #[test]
    fn a_project_has_an_interface_when_its_brain_decides_a_design() {
        assert!(!has_ui(&[rule("prd", "Qué es: una API")]));
        assert!(has_ui(&[rule("dis", "Tipo de app: web")]));
        assert!(!has_ui(&[rule("dis", "Tipo de app: sin interfaz")]));
    }

    #[test]
    fn the_design_assets_go_into_the_worktree_where_agents_can_open_them() {
        let root = std::env::temp_dir().join(format!("hangar-design-{}", uuid::Uuid::new_v4().simple()));
        let wt = root.join("wt");
        std::fs::create_dir_all(refs_dir(&root, "p1")).unwrap();
        std::fs::create_dir_all(approved_dir(&root, "p1")).unwrap();
        std::fs::write(refs_dir(&root, "p1").join("linear.png"), b"x").unwrap();
        std::fs::write(approved_dir(&root, "p1").join("movil-claro.png"), b"x").unwrap();
        let files = stage_assets(&root, "p1", &wt);
        assert_eq!(files, vec![".hangar/design/aprobado/movil-claro.png".to_string(), ".hangar/design/refs/linear.png".to_string()]);
        assert!(wt.join(".hangar/design/refs/linear.png").is_file());
        assert!(assets_prompt(&files).contains("refs/linear.png"));
        assert!(assets_prompt(&[]).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_ais_directions_are_read_with_their_preview() {
        let answer = json!({"directions": [
            {"name":"Sobria","mood":"mucho aire","colors":["primario #4F46E5","fondo #FFFFFF"],"font":"Inter","shape":"radio 12px","library":"shadcn/ui","dark":true,"preview_html":"<html><body>hola</body></html>"},
            {"name":"rota"}
        ]});
        let ds = read_directions(&answer);
        assert_eq!(ds.len(), 1, "an incomplete direction is dropped");
        assert_eq!((ds[0].name.as_str(), ds[0].preview_html.as_str()), ("Sobria", "<html><body>hola</body></html>"));
        assert!(directions_prompt("Pokédex", "- Qué es: una pokédex", &[]).contains("3 direcciones"));
    }

    /// The real browser, when there is one: four shots, and «light» is light
    /// even on a Mac in dark mode.
    #[test]
    fn the_style_guide_is_captured_in_every_size_and_theme() {
        if chrome().is_none() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("hangar-shots-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let page = dir.join("p.html");
        std::fs::write(&page, "<html><style>body{margin:0;background:#fff}@media (prefers-color-scheme: dark){body{background:#000}}</style><body></body></html>").unwrap();
        let shots = capture("p-shots-tests", &format!("file://{}", page.display()), &dir.join("out")).unwrap();
        assert_eq!(shots.len(), 4);
        let light = std::fs::read(&shots[0].1).unwrap();
        let dark = std::fs::read(&shots[2].1).unwrap();
        assert_ne!(light, dark, "the light shot isn't the dark one");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shots_have_file_names_a_shell_can_live_with() {
        assert_eq!(shot_file("móvil · oscuro"), "movil-oscuro.png");
        assert_eq!(shot_file("escritorio · claro"), "escritorio-claro.png");
    }
}
