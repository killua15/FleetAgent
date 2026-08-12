//! Repositorios de GitHub de los que instalar skills.
//!
//! Todo esto envuelve `gh skill`, que **ya está** entre las dependencias de
//! Hangar (PRs, issues). No se escribe descarga, ni resolución de versiones, ni
//! seguimiento de origen: `gh` lo hace, e inyecta en el frontmatter los
//! metadatos (`github-repo`, `github-tree-sha`) que después permiten
//! actualizarla — los mismos que `skills::preserved_frontmatter` ya conserva
//! cuando editas la skill desde la app.
//!
//! Un repo privado funciona igual, porque `gh` habla autenticado como el
//! usuario. Es justo el caso de un repo de skills de equipo.
//!
//! **`gh skill` está en preview** y su propia ayuda avisa de que puede cambiar
//! sin aviso. Por eso el parseo de su salida vive en un solo sitio, con tests:
//! el día que cambie el formato, falla aquí y no en seis pantallas.

use crate::forges::github::{available_and_authed as gh_available_and_authed, gh_output};
use crate::db::DbConnection;
use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;

/// Descargar tarda más que listar, y el timeout compartido (25 s) se queda
/// corto para un repo con muchas skills.
const INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSource {
    pub id: String,
    /// Siempre `owner/repo`, nunca una URL.
    pub repo: String,
    pub label: String,
}

/// Una skill vista en el repo, todavía sin instalar.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSkill {
    pub name: String,
    pub description: String,
    /// Si ya está en la biblioteca. Sin esto la lista no sabe distinguir entre
    /// «instalar» y «reinstalar», que son cosas distintas para quien mira.
    pub installed: bool,
}

/// `https://github.com/x/y`, `github.com/x/y.git` y `x/y` son el mismo sitio.
///
/// Normalizar al guardar es lo que evita dos fuentes para un mismo repo, que
/// luego aparecerían dos veces en la lista sin explicación.
pub(crate) fn normalize_repo(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('/');
    let without_scheme = trimmed
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.")
        .trim_start_matches("github.com/");
    let cleaned = without_scheme.trim_end_matches(".git");

    let parts: Vec<&str> = cleaned.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() != 2 {
        return Err(format!("'{raw}' no parece un repositorio — usa `owner/repo` o su URL de GitHub"));
    }
    let ok = |s: &str| s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if !ok(parts[0]) || !ok(parts[1]) {
        return Err(format!("'{raw}' tiene caracteres que un repositorio de GitHub no puede llevar"));
    }
    Ok(format!("{}/{}", parts[0], parts[1]))
}

/// Lee el listado que `gh skill install <repo>` imprime cuando no se le nombra
/// ninguna skill: `<nombre>\t<descripción>` por línea.
///
/// Tolerante a propósito. `gh` intercala líneas informativas («Using ref
/// main…», «Note: found 11 namespaced skills…») y una descripción puede traer
/// comillas, dos puntos y hasta tabuladores. Se parte por el **primer**
/// tabulador y se ignora lo que no tenga esa forma, en vez de fallar y dejar la
/// pantalla vacía por una línea de cortesía.
pub(crate) fn parse_skill_listing(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter_map(|line| {
            let (name, description) = line.split_once('\t')?;
            let name = name.trim();
            if name.is_empty() || name.contains(' ') {
                // Una línea informativa no lleva tabulador, pero si alguna vez
                // lo lleva, un "nombre" con espacios la delata: un nombre de
                // skill es un nombre de carpeta.
                return None;
            }
            Some((name.to_string(), description.trim().to_string()))
        })
        .collect()
}

// ------------------------------------------------------------------ fuentes

fn row_to_source(row: &rusqlite::Row) -> rusqlite::Result<SkillSource> {
    Ok(SkillSource { id: row.get("id")?, repo: row.get("repo")?, label: row.get("label")? })
}

#[tauri::command]
pub fn list_skill_sources(db: State<DbConnection>) -> Result<Vec<SkillSource>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT * FROM skill_sources ORDER BY created_at ASC").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], row_to_source).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn add_skill_source(db: State<DbConnection>, repo: String, label: Option<String>) -> Result<SkillSource, String> {
    let repo = normalize_repo(&repo)?;
    let label = label.map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).unwrap_or_else(|| repo.clone());
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // `ON CONFLICT DO NOTHING` y luego leer: añadir un repo que ya está no es un
    // error que merezca un aviso rojo, es un no-op.
    conn.execute(
        "INSERT INTO skill_sources (id, repo, label, created_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(repo) DO NOTHING",
        params![Uuid::new_v4().to_string(), repo, label, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    conn.query_row("SELECT * FROM skill_sources WHERE repo = ?1", params![repo], row_to_source).map_err(|e| e.to_string())
}

/// Quita el repo de la lista. **No toca las skills ya instaladas**: desde que
/// aterrizaron en la biblioteca son del usuario, y borrarlas por dar de baja su
/// origen sería una sorpresa desagradable.
#[tauri::command]
pub fn remove_skill_source(db: State<DbConnection>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM skill_sources WHERE id = ?1", params![id]).map_err(|e| e.to_string())?;
    Ok(())
}

// ------------------------------------------------------------------- gh skill

#[tauri::command]
pub fn list_source_skills<R: tauri::Runtime>(app: tauri::AppHandle<R>, repo: String) -> Result<Vec<RemoteSkill>, String> {
    gh_available_and_authed()?;
    let repo = normalize_repo(&repo)?;
    // Sin nombre de skill, `gh skill install` no instala: lista.
    let output = gh_output(None, &["skill", "install", &repo])?;
    if !output.status.success() {
        return Err(format!("no se pudo leer {repo}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let installed = crate::commands::skills::list_skills(app)?;
    Ok(parse_skill_listing(&String::from_utf8_lossy(&output.stdout))
        .into_iter()
        .map(|(name, description)| RemoteSkill {
            installed: installed.iter().any(|s| s.name == name),
            name,
            description,
        })
        .collect())
}

/// El árbol de ficheros y el contenido de una skill, **antes** de instalarla.
///
/// Este paso es obligatorio en la interfaz y no es un adorno: una skill parece
/// documentación —es un markdown— pero puede pre-autorizar la ejecución de sus
/// propios scripts vía `allowed-tools`. El marketplace de JetBrains revisa y
/// firma lo que publica; una búsqueda de código en GitHub no revisa nada, así
/// que esta pantalla es lo único que hay entre el usuario y esa diferencia.
#[tauri::command]
pub fn preview_source_skill(repo: String, skill: String) -> Result<String, String> {
    gh_available_and_authed()?;
    let repo = normalize_repo(&repo)?;
    let output = gh_output(None, &["skill", "preview", &repo, &skill])?;
    if !output.status.success() {
        return Err(format!("no se pudo previsualizar {skill}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn gh_with_timeout(args: &[&str], timeout: std::time::Duration) -> Result<std::process::Output, String> {
    let mut cmd = std::process::Command::new(crate::providers::resolve_binary("gh"));
    cmd.args(args);
    crate::process_timeout::run_with_timeout(cmd, timeout).map_err(|e| format!("gh {}: {e}", args.join(" ")))
}

#[tauri::command]
pub fn install_source_skill<R: tauri::Runtime>(app: tauri::AppHandle<R>, repo: String, skill: String) -> Result<(), String> {
    gh_available_and_authed()?;
    let repo = normalize_repo(&repo)?;
    let library = crate::commands::skills::library_dir(&crate::app_data::root(&app)?);
    std::fs::create_dir_all(&library).map_err(|e| format!("could not create {}: {e}", library.display()))?;
    let dir = library.to_string_lossy().to_string();

    // `--force` porque la UI ya preguntó: llegar hasta aquí significa que el
    // usuario vio el preview y confirmó.
    let output = gh_with_timeout(&["skill", "install", &repo, &skill, "--dir", &dir, "--force"], INSTALL_TIMEOUT)?;
    if !output.status.success() {
        return Err(format!("no se pudo instalar {skill}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

/// Qué cambió en el origen, **sin tocar nada**.
///
/// Instalar copia, no enlaza: una skill se queda en la versión que trajiste
/// hasta que la actualices. Es lo correcto —el repo de otro no debería cambiar
/// tu comportamiento sin avisar— pero significa que hace falta una forma de
/// preguntar, y esta es.
#[tauri::command]
pub fn check_skill_updates<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<String, String> {
    gh_available_and_authed()?;
    let library = crate::commands::skills::library_dir(&crate::app_data::root(&app)?);
    if !library.is_dir() {
        return Ok(String::new());
    }
    let dir = library.to_string_lossy().to_string();
    let output = gh_output(None, &["skill", "update", "--dir", &dir, "--dry-run"])?;
    // `gh` escribe el informe en stdout y las notas en stderr; se muestran los
    // dos, porque «no hay nada que actualizar» a veces sale por el segundo.
    let mut report = String::from_utf8_lossy(&output.stdout).to_string();
    let notes = String::from_utf8_lossy(&output.stderr);
    if !notes.trim().is_empty() {
        report.push_str(notes.trim());
    }
    Ok(report)
}

#[tauri::command]
pub fn update_installed_skills<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<String, String> {
    gh_available_and_authed()?;
    let library = crate::commands::skills::library_dir(&crate::app_data::root(&app)?);
    let dir = library.to_string_lossy().to_string();
    let output = gh_with_timeout(&["skill", "update", "--dir", &dir, "--all"], INSTALL_TIMEOUT)?;
    if !output.status.success() {
        return Err(format!("no se pudieron actualizar: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_and_the_short_form_are_the_same_repo() {
        // Si no, pegar la URL y escribir `x/y` crearían dos fuentes para el
        // mismo sitio y la lista mostraría el repo dos veces.
        for raw in [
            "killua15/mis-skills",
            "https://github.com/killua15/mis-skills",
            "http://github.com/killua15/mis-skills",
            "github.com/killua15/mis-skills",
            "https://www.github.com/killua15/mis-skills/",
            "https://github.com/killua15/mis-skills.git",
        ] {
            assert_eq!(normalize_repo(raw).unwrap(), "killua15/mis-skills", "falló con {raw}");
        }
    }

    #[test]
    fn something_that_is_not_a_repo_is_refused() {
        assert!(normalize_repo("").is_err());
        assert!(normalize_repo("solo-un-nombre").is_err());
        assert!(normalize_repo("a/b/c").is_err());
        assert!(normalize_repo("owner/repo; rm -rf /").is_err());
    }

    /// La salida de `gh skill` es la parte frágil: su ayuda avisa de que el
    /// comando está en preview y puede cambiar sin aviso.
    #[test]
    fn the_listing_survives_the_noise_gh_prints_around_it() {
        let stdout = "Using ref main (a7fdcd50)\n\
             Note: found 11 namespaced skill(s) in skills/{author}/ directories\n\
             code-review\tDeep code audit. Triggers on: \"code review\", \"audit the code\".\n\
             \n\
             add-comments\tAdd educational comments to code files.\n";
        let parsed = parse_skill_listing(stdout);
        assert_eq!(parsed.len(), 2, "las líneas informativas no son skills: {parsed:?}");
        assert_eq!(parsed[0].0, "code-review");
        assert!(parsed[0].1.contains("\"code review\""), "las comillas de la descripción sobreviven");
        assert_eq!(parsed[1].0, "add-comments");
    }

    #[test]
    fn a_description_with_its_own_tab_keeps_its_name() {
        // Se parte por el PRIMER tabulador; los demás son de la descripción.
        let parsed = parse_skill_listing("deploy\tprimero\tsegundo\n");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0, "deploy");
        assert_eq!(parsed[0].1, "primero\tsegundo");
    }

    /// Contra salida **real** de `gh skill install github/awesome-copilot`,
    /// grabada en `tests/fixtures/`.
    ///
    /// Es el test que importa de los de aquí: `gh skill` está en preview y su
    /// ayuda avisa de que puede cambiar sin aviso. Los casos sintéticos de
    /// arriba comprueban lo que yo *creo* que imprime; este comprueba lo que
    /// imprimió de verdad. Si el formato cambia, falla aquí.
    ///
    /// La mezcla no es casual: 14 skills entre 26 líneas de ruido —cabeceras de
    /// `gh` y continuaciones de descripciones multilínea, que son listas de
    /// viñetas partidas en varias líneas.
    #[test]
    fn the_parser_handles_real_gh_output() {
        let raw = std::fs::read_to_string("tests/fixtures/real_gh_skill_listing.txt").expect("read fixture");
        let parsed = parse_skill_listing(&raw);
        assert_eq!(parsed.len(), 14, "salieron {:?}", parsed.iter().map(|(n, _)| n).collect::<Vec<_>>());

        // Nombres de carpeta de verdad, no fragmentos de una descripción.
        for (name, _) in &parsed {
            assert!(!name.contains(' '), "'{name}' no es un nombre de skill");
            assert!(!name.is_empty());
        }
        assert!(parsed.iter().any(|(n, _)| n == "add-educational-comments"), "{parsed:?}");
        // Una descripción larga se queda en su primera línea, y basta: la lista
        // es un resumen y el contenido entero se ve en el preview.
        assert!(parsed.iter().all(|(_, d)| !d.is_empty()), "ninguna descripción vacía");
    }

    #[test]
    fn an_empty_listing_is_not_an_error() {
        assert!(parse_skill_listing("").is_empty());
        assert!(parse_skill_listing("Using ref main\n").is_empty());
    }
}
