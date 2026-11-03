//! The brain as the creator's decisions. Each section has fixed fields
//! (what's always decided: the package manager, the branches, what the
//! product leaves out…) and free suggestions — the creator's, or the AI's.
//! The AI reads the repo and proposes; only what the creator confirms goes
//! into the brain. Where a field can be checked, the Factory writes its check
//! from a tested template: the AI never writes a shell command for it.

use std::time::Duration;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};

use super::agent::{AgentCall, CliRunner, Outcome, Runner};
use super::brain::{self, Report, Rule};
use crate::db::DbConnection;

/// One fixed field of a section.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub key: &'static str,
    pub section: &'static str,
    pub label: &'static str,
    pub help: &'static str,
    /// `text`, `list` (one per line), `number` or `choice`.
    pub kind: &'static str,
    pub options: &'static [&'static str],
    /// The weight it starts with: `must` or `should`.
    pub level: &'static str,
    /// The Factory writes its check.
    pub guarded: bool,
    /// Decided by how the Factory works: shown, not edited.
    pub fixed: Option<&'static str>,
}

const fn f(key: &'static str, section: &'static str, label: &'static str, help: &'static str, kind: &'static str, level: &'static str, guarded: bool) -> Field {
    Field { key, section, label, help, kind, options: &[], level, guarded, fixed: None }
}

const MANAGERS: &[&str] = &["npm", "pnpm", "yarn", "bun", "uv", "poetry", "pip", "cargo", "go"];
const LANGS: &[&str] = &["inglés", "español"];
const COMMITS: &[&str] = &["convencionales (feat:, fix:…)", "libres"];

/// The fields, section by section, in the order the brain lists them.
pub fn template() -> Vec<Field> {
    vec![
        f("prd.que", "prd", "Qué es", "El sistema en una o dos frases.", "text", "must", false),
        f("prd.para_quien", "prd", "Para quién", "Quién lo usa y para qué.", "text", "should", false),
        f("prd.entra", "prd", "Qué entra", "Lo que el sistema hace. Una cosa por línea.", "list", "must", false),
        f("prd.no_entra", "prd", "Qué NO entra", "Lo que la IA no debe construir aunque parezca útil. Una por línea.", "list", "must", false),
        f("prd.glosario", "prd", "Glosario", "Término: significado. Uno por línea.", "list", "should", false),
        f("arq.capas", "arq", "Módulos y capas", "Cada módulo o capa y para qué es. Uno por línea.", "list", "must", false),
        f("arq.carpetas", "arq", "Carpetas fijas", "Carpetas que deben existir (src/api, src/components…). Una por línea.", "list", "must", true),
        f("arq.imports", "arq", "Importaciones prohibidas", "«origen → destino»: lo que una carpeta no puede importar (src/components → src/api). Una por línea.", "list", "must", true),
        f("arq.no_tocar", "arq", "Lo que no se toca", "Rutas que ninguna orden puede cambiar. Una por línea.", "list", "must", true),
        f("tec.lenguaje", "tec", "Lenguaje", "TypeScript 5, Python 3.12…", "text", "must", false),
        f("tec.framework", "tec", "Framework", "React 19, FastAPI…", "text", "must", false),
        Field { options: MANAGERS, ..f("tec.gestor", "tec", "Gestor de paquetes", "Solo este, con su lockfile versionado.", "choice", "must", true) },
        f("tec.versiones", "tec", "Versiones exactas", "paquete@versión, una por línea (react@19.1.0). Para proyectos con package.json.", "list", "must", true),
        f("tec.permitidas", "tec", "Librerías permitidas", "Las que se pueden añadir sin preguntar. Una por línea.", "list", "should", false),
        f("tec.prohibidas", "tec", "Librerías prohibidas", "Las que no pueden aparecer. Una por línea.", "list", "must", true),
        f("ent.runtime", "ent", "Runtime", "«node 24», «python 3.12», o el nombre de un programa. Uno por línea.", "list", "must", true),
        f("ent.navegadores", "ent", "Navegadores de los tests", "chromium, firefox o webkit. Uno por línea: se comprueba que arrancan.", "list", "must", true),
        f("ent.servicios", "ent", "Servicios", "Base de datos, Docker… Uno por línea.", "list", "should", false),
        Field { options: LANGS, ..f("cod.idioma_comentarios", "cod", "Idioma de los comentarios", "", "choice", "should", false) },
        Field { options: LANGS, ..f("cod.idioma_nombres", "cod", "Idioma de los nombres", "Variables, funciones, archivos.", "choice", "should", false) },
        f("cod.estilo_nombres", "cod", "Estilo de nombres", "camelCase para funciones, PascalCase para componentes…", "text", "should", false),
        f("cod.modulos", "cod", "Cómo se modulariza", "Un componente por archivo, un hook por archivo…", "text", "should", false),
        f("cod.max_lineas", "cod", "Máximo de líneas por archivo", "Número.", "number", "should", true),
        f("cod.errores", "cod", "Manejo de errores", "Cómo se tratan y se muestran.", "text", "should", false),
        Field { fixed: Some("dev"), ..f("git.base", "git", "Rama base", "Cada orden se corta de aquí.", "text", "must", false) },
        Field { fixed: Some("feat/<CLAVE>-<slug>"), ..f("git.ramas", "git", "Ramas", "Las crea la Factory, así que el check coincide siempre.", "text", "must", true) },
        Field { options: COMMITS, ..f("git.commits", "git", "Formato de los commits", "", "choice", "should", true) },
        Field { fixed: Some("merge --no-ff en dev, en local"), ..f("git.fusion", "git", "Cómo se fusiona", "", "text", "must", false) },
        f("dep.donde", "dep", "Dónde se despliega", "Vercel, un servidor propio, nada todavía…", "text", "should", false),
        f("dep.como", "dep", "Cómo", "Manual, CI/CD…", "text", "should", false),
        f("dep.entornos", "dep", "Entornos", "Uno por línea.", "list", "should", false),
        f("dep.antes", "dep", "Antes de desplegar", "Lo que tiene que pasar antes.", "text", "should", false),
        f("cal.hecho", "cal", "Qué significa «hecho»", "Cuándo una orden está terminada.", "text", "must", false),
        f("cal.suite", "cal", "Comando de la suite", "Uno solo: npm run verify, pytest… La puerta lo corre en cada orden.", "text", "must", true),
        f("cal.cobertura", "cal", "Cobertura mínima", "Porcentaje; lo comprueba la propia suite.", "number", "should", false),
        f("rsg.preguntar", "rsg", "Cuándo la IA para y pregunta", "Cambiar una versión fijada, borrar datos… Uno por línea.", "list", "must", false),
        f("rsg.tamano", "rsg", "Tamaño máximo de una orden", "Líneas de diff (añadidas + borradas), sin lockfiles.", "number", "should", true),
    ]
}

/// One decision or suggestion of the draft.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub section: String,
    /// A template field, or `None` for a free suggestion.
    #[serde(default)]
    pub field: Option<String>,
    pub value: String,
    /// `must` or `should`.
    pub level: String,
    /// A free item's own check (a field's comes from its template).
    #[serde(default)]
    pub check: Option<String>,
    /// `proposed` (by the AI, waiting), `confirmed` (by the creator) or
    /// `rejected` (kept, so the AI doesn't propose it again).
    pub status: String,
    /// `ai`, `creator`, `brain` (imported from a sealed version) or `inbox`.
    pub by: String,
    #[serde(default)]
    pub why: Option<String>,
    /// The rule id it is sealed as; kept across versions.
    #[serde(default)]
    pub rule_id: Option<String>,
}

/// The draft as the editor shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftView {
    pub fields: Vec<Field>,
    pub items: Vec<ItemView>,
    /// The brain was sealed some other way since: the draft is behind.
    pub stale: bool,
    pub based_on: Option<i64>,
    pub brain_version: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemView {
    #[serde(flatten)]
    pub item: Item,
    /// The check this decision becomes, as the gate would run it.
    pub guard: Option<String>,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn lines(v: &str) -> Vec<String> {
    v.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

fn is_node(manager: Option<&str>) -> bool {
    matches!(manager, Some("npm" | "pnpm" | "yarn" | "bun") | None)
}

/// The check a field's value becomes. `manager` is the decided package
/// manager, which some checks depend on.
pub fn guard(field: &str, value: &str, manager: Option<&str>) -> Option<String> {
    let value = value.trim();
    match field {
        "arq.carpetas" => {
            let dirs = lines(value);
            (!dirs.is_empty()).then(|| format!("for d in {}; do test -d \"$d\" || {{ echo \"falta la carpeta $d\" >&2; exit 1; }}; done", dirs.iter().map(|d| quote(d)).collect::<Vec<_>>().join(" ")))
        }
        "arq.imports" => {
            let pairs: Vec<(String, String)> = lines(value)
                .iter()
                .filter_map(|l| {
                    let (a, b) = l.split_once('→').or_else(|| l.split_once("->"))?;
                    let (a, b) = (a.trim(), b.trim().trim_end_matches('/'));
                    let seg = b.rsplit('/').next().unwrap_or(b);
                    (!a.is_empty() && !seg.is_empty()).then(|| (a.to_string(), seg.to_string()))
                })
                .collect();
            (!pairs.is_empty()).then(|| {
                pairs
                    .iter()
                    .map(|(a, seg)| format!("! grep -rnE \"from ['\\\"]([^'\\\"]*/)?{seg}(/[^'\\\"]*)?['\\\"]\" {} 2>/dev/null", quote(a)))
                    .collect::<Vec<_>>()
                    .join(" && ")
            })
        }
        "arq.no_tocar" => {
            let paths = lines(value);
            (!paths.is_empty()).then(|| format!("test -z \"$(git diff --name-only dev...HEAD -- {})\"", paths.iter().map(|p| quote(p)).collect::<Vec<_>>().join(" ")))
        }
        "tec.gestor" => {
            let (keep, others): (&str, &[&str]) = match value {
                "npm" => ("package-lock.json", &["pnpm-lock.yaml", "yarn.lock", "bun.lockb", "bun.lock"]),
                "pnpm" => ("pnpm-lock.yaml", &["package-lock.json", "yarn.lock", "bun.lockb", "bun.lock"]),
                "yarn" => ("yarn.lock", &["package-lock.json", "pnpm-lock.yaml", "bun.lockb", "bun.lock"]),
                "bun" => return Some("{ test -f bun.lock || test -f bun.lockb; } && test ! -e package-lock.json && test ! -e pnpm-lock.yaml && test ! -e yarn.lock".into()),
                "uv" => ("uv.lock", &["poetry.lock"]),
                "poetry" => ("poetry.lock", &["uv.lock"]),
                "pip" => ("requirements.txt", &[]),
                "cargo" => ("Cargo.lock", &[]),
                "go" => ("go.mod", &[]),
                _ => return None,
            };
            let mut c = format!("test -f {keep}");
            for o in others {
                c.push_str(&format!(" && test ! -e {o}"));
            }
            Some(c)
        }
        "tec.versiones" => {
            if !is_node(manager) {
                return None;
            }
            let want: Vec<(String, String)> = lines(value)
                .iter()
                .filter_map(|l| {
                    let at = l.rfind('@').filter(|i| *i > 0)?;
                    Some((l[..at].trim().to_string(), l[at + 1..].trim().to_string()))
                })
                .filter(|(n, v)| !n.is_empty() && !v.is_empty() && !n.contains('\'') && !v.contains('\''))
                .collect();
            (!want.is_empty()).then(|| {
                let obj = want.iter().map(|(n, v)| format!("{}:{}", json!(n), json!(v))).collect::<Vec<_>>().join(",");
                format!(
                    "node -e 'const p=require(\"./package.json\");const d={{...p.dependencies,...p.devDependencies}};const w={{{obj}}};const bad=Object.entries(w).filter(([k,v])=>d[k]!==v);if(bad.length){{console.error(bad.map(([k,v])=>k+\" \"+(d[k]||\"falta\")+\" ≠ \"+v).join(\"\\n\"));process.exit(1)}}'"
                )
            })
        }
        "tec.prohibidas" => {
            let names: Vec<String> = lines(value).into_iter().filter(|n| !n.contains('\'')).collect();
            if names.is_empty() {
                return None;
            }
            if is_node(manager) {
                let list = names.iter().map(|n| json!(n).to_string()).collect::<Vec<_>>().join(",");
                Some(format!("node -e 'const p=require(\"./package.json\");const d={{...p.dependencies,...p.devDependencies}};const bad=[{list}].filter(n=>n in d);if(bad.length){{console.error(\"prohibidas: \"+bad.join(\", \"));process.exit(1)}}'"))
            } else {
                let files = "requirements.txt pyproject.toml Cargo.toml go.mod";
                Some(names.iter().map(|n| format!("! grep -qsw {} {files}", quote(n))).collect::<Vec<_>>().join(" && "))
            }
        }
        "ent.runtime" => {
            let parts: Vec<String> = lines(value)
                .iter()
                .map(|l| {
                    let mut w = l.split_whitespace();
                    let name = w.next().unwrap_or("").to_lowercase();
                    let ver = w.next().map(|v| v.trim_start_matches('v').to_string());
                    match (name.as_str(), ver) {
                        ("node" | "nodejs", Some(v)) => format!("node -v | grep -E '^v{}(\\.|$)' > /dev/null", regex::escape(&v)),
                        ("python" | "python3", Some(v)) => format!("python3 --version 2>&1 | grep -E 'Python {}(\\.|$)' > /dev/null", regex::escape(&v)),
                        (n, _) => format!("command -v {} > /dev/null", quote(n)),
                    }
                })
                .collect();
            (!parts.is_empty()).then(|| parts.join(" && "))
        }
        "ent.navegadores" => {
            let names: Vec<String> = lines(value).iter().map(|b| b.to_lowercase()).filter(|b| matches!(b.as_str(), "chromium" | "firefox" | "webkit")).collect();
            (!names.is_empty()).then(|| {
                names
                    .iter()
                    .map(|b| {
                        format!(
                            "if [ -d node_modules/@playwright/test ]; then node -e 'require(\"@playwright/test\").{b}.launch().then(x=>x.close()).catch(e=>{{console.error(\"{b} no arranca: \"+e.message);process.exit(1)}})'; else ls -d \"${{PLAYWRIGHT_BROWSERS_PATH:-$HOME/Library/Caches/ms-playwright}}\"/{b}-* > /dev/null 2>&1 || ls -d \"$HOME/.cache/ms-playwright\"/{b}-* > /dev/null 2>&1; fi"
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" && ")
            })
        }
        "cod.max_lineas" => {
            let n: u32 = value.parse().ok()?;
            Some(format!("! git ls-files | grep -E '\\.(ts|tsx|js|jsx|mjs|py|rs|go|kt|java|swift|dart|vue|svelte)$' | xargs wc -l 2>/dev/null | awk '$2!=\"total\" && $1>{n}' | grep . > /dev/null"))
        }
        "git.ramas" => Some("git branch --show-current | grep -E '^((feat|fix|chore)/[A-Za-z0-9]+-[0-9]+-[a-z0-9-]+|main|dev)$' > /dev/null".into()),
        "git.commits" if value.starts_with("convencionales") => {
            Some("! git log --no-merges --format=%s dev..HEAD | grep -vE '^(feat|fix|test|chore|refactor|docs|build|style|perf|ci|revert)(\\([^)]+\\))?!?: .+' > /dev/null".into())
        }
        "cal.suite" => (!value.is_empty()).then(|| value.to_string()),
        "rsg.tamano" => {
            let n: u32 = value.parse().ok()?;
            Some(format!(
                "test \"$(git diff --numstat dev...HEAD -- . ':!package-lock.json' ':!pnpm-lock.yaml' ':!yarn.lock' ':!bun.lock' ':!uv.lock' ':!poetry.lock' ':!Cargo.lock' | awk '{{s+=$1+$2}} END{{print s+0}}')\" -le {n}"
            ))
        }
        _ => None,
    }
}

/// The value a fixed field always has.
fn effective_value(def: &Field, item: Option<&Item>) -> Option<String> {
    def.fixed.map(str::to_string).or_else(|| item.map(|i| i.value.clone()))
}

fn manager_of(items: &[Item]) -> Option<String> {
    items.iter().find(|i| i.field.as_deref() == Some("tec.gestor") && i.status == "confirmed").map(|i| i.value.trim().to_string())
}

fn section_prefix(section: &str) -> &'static str {
    brain::SECTIONS.iter().find(|(k, _, _)| *k == section).map(|(_, _, p)| *p).unwrap_or("REG")
}

/// The fields the Factory itself decides (the base branch, the branches,
/// how it merges) are always in the draft, confirmed.
pub fn ensure_fixed(items: &mut Vec<Item>) {
    for def in template().into_iter().filter(|d| d.fixed.is_some()) {
        if !items.iter().any(|i| i.field.as_deref() == Some(def.key)) {
            items.push(Item {
                id: uuid::Uuid::new_v4().to_string(),
                section: def.section.into(),
                field: Some(def.key.into()),
                value: def.fixed.unwrap_or_default().into(),
                level: def.level.into(),
                check: None,
                status: "confirmed".into(),
                by: "factory".into(),
                why: None,
                rule_id: None,
            });
        }
    }
}

/// Gives every confirmed item without one a rule id, the next free in its
/// section. An id stays with its decision across versions: decisions are
/// cited by it. (A deleted decision's id can go to a new one; the ratchet
/// keys on id and command, so it starts over.)
pub fn assign_ids(items: &mut Vec<Item>) {
    ensure_fixed(items);
    let mut used: std::collections::HashSet<String> = items.iter().filter_map(|i| i.rule_id.clone()).collect();
    let order: Vec<&str> = template().iter().map(|f| f.key).collect();
    let mut idx: Vec<usize> = (0..items.len()).collect();
    // Fields in template order, then suggestions as they came.
    idx.sort_by_key(|&i| (items[i].field.as_deref().and_then(|k| order.iter().position(|o| *o == k)).unwrap_or(usize::MAX), i));
    for i in idx {
        if items[i].status != "confirmed" || items[i].rule_id.is_some() {
            continue;
        }
        let p = section_prefix(&items[i].section);
        let mut n = 1;
        while used.contains(&format!("{p}-{n:02}")) {
            n += 1;
        }
        let id = format!("{p}-{n:02}");
        used.insert(id.clone());
        items[i].rule_id = Some(id);
    }
}

/// The confirmed decisions as brain rules, fixed fields included.
pub fn render_rules(items: &[Item]) -> Vec<Rule> {
    let defs = template();
    let manager = manager_of(items);
    let mut rules = Vec::new();
    for def in &defs {
        let item = items.iter().find(|i| i.field.as_deref() == Some(def.key) && i.status == "confirmed");
        if def.fixed.is_none() && item.is_none() {
            continue;
        }
        let Some(value) = effective_value(def, item).filter(|v| !v.trim().is_empty()) else { continue };
        let Some(id) = item.and_then(|i| i.rule_id.clone()) else { continue };
        let shown = if def.kind == "list" { lines(&value).join("; ") } else { value.clone() };
        let check = if def.guarded { guard(def.key, &value, manager.as_deref()) } else { None };
        rules.push(Rule { id, section: def.section.into(), level: item.map(|i| i.level.clone()).unwrap_or_else(|| def.level.into()), text: format!("{}: {shown}", def.label), check });
    }
    for i in items.iter().filter(|i| i.field.is_none() && i.status == "confirmed" && !i.value.trim().is_empty()) {
        rules.push(Rule {
            id: i.rule_id.clone().unwrap_or_default(),
            section: i.section.clone(),
            level: i.level.clone(),
            text: i.value.trim().to_string(),
            check: i.check.clone().filter(|c| !c.trim().is_empty()),
        });
    }
    rules
}

/// A draft out of a sealed brain: every rule a confirmed suggestion, with
/// its id — where to go on from when the brain was made another way.
pub fn from_sealed(rules: &[Rule]) -> Vec<Item> {
    rules
        .iter()
        .map(|r| Item {
            id: uuid::Uuid::new_v4().to_string(),
            section: r.section.clone(),
            field: None,
            value: r.text.clone(),
            level: r.level.clone(),
            check: r.check.clone(),
            status: "confirmed".into(),
            by: "brain".into(),
            why: None,
            rule_id: Some(r.id.clone()),
        })
        .collect()
}

pub fn load_draft(conn: &Connection, project_id: &str) -> Result<Option<(Vec<Item>, Option<i64>)>, String> {
    let row: Option<(String, Option<i64>)> = conn
        .query_row("SELECT items, based_on FROM factory_brain_draft WHERE project_id = ?1", params![project_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()
        .map_err(|e| e.to_string())?;
    Ok(row.map(|(json, based)| (serde_json::from_str(&json).unwrap_or_default(), based)))
}

pub fn save_draft(conn: &Connection, project_id: &str, items: &[Item], based_on: Option<i64>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO factory_brain_draft (project_id, items, based_on, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project_id) DO UPDATE SET items = excluded.items, based_on = excluded.based_on, updated_at = excluded.updated_at",
        params![project_id, serde_json::to_string(items).unwrap_or_else(|_| "[]".into()), based_on, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn current_version(conn: &Connection, project_id: &str) -> Option<i64> {
    conn.query_row("SELECT brain_version FROM factory_projects WHERE project_id = ?1", params![project_id], |r| r.get(0)).ok().flatten()
}

/// The draft, started from the sealed brain the first time.
pub fn view(conn: &Connection, project_id: &str) -> Result<DraftView, String> {
    let version = current_version(conn, project_id);
    let (mut items, based_on) = match load_draft(conn, project_id)? {
        Some(d) => d,
        None => {
            let items = match brain::load(conn, project_id, None)? {
                Some(sealed) => from_sealed(&sealed.rules),
                None => Vec::new(),
            };
            (items, version)
        }
    };
    if !items.iter().any(|i| i.by == "factory") {
        assign_ids(&mut items);
        save_draft(conn, project_id, &items, based_on)?;
    }
    let manager = manager_of(&items);
    let items = items
        .into_iter()
        .map(|i| {
            let guard = match i.field.as_deref() {
                Some(k) if template().iter().any(|d| d.key == k && d.guarded) => guard(k, &i.value, manager.as_deref()),
                Some(_) => None,
                None => i.check.clone(),
            };
            ItemView { item: i, guard }
        })
        .collect();
    Ok(DraftView { fields: template(), items, stale: version.is_some() && based_on != version, based_on, brain_version: version })
}

// ---------------------------------------------------------------- the AI

pub fn propose_schema() -> Value {
    json!({
        "type": "object",
        "required": ["fields", "suggestions"],
        "properties": {
            "fields": { "type": "array", "items": { "type": "object", "required": ["key", "value", "why"], "properties": {
                "key": {"type":"string"}, "value": {"type":"string"}, "why": {"type":"string"} } } },
            "suggestions": { "type": "array", "items": { "type": "object", "required": ["section", "text", "level", "why"], "properties": {
                "section": {"type":"string","enum":["prd","arq","tec","ent","cod","dat","sec","cal","git","dep","rsg"]},
                "text": {"type":"string"}, "level": {"type":"string","enum":["must","should"]}, "why": {"type":"string"} } } }
        }
    })
}

pub fn propose_prompt(project: &str, items: &[Item]) -> String {
    let defs = template();
    let mut p = format!(
        "Ayudas al creador de «{project}» a decidir las bases del proyecto. Lee el repo (si está vacío, propón lo razonable para lo que veas) y PROPÓN; el creador decide, tú no.\n\n\
         1) Para cada campo de abajo que esté vacío, propón un valor solo si el repo lo muestra o es una recomendación clara; si no, no lo incluyas. Formato del valor: el que pide cada campo (listas: una cosa por línea).\n\
         2) Sugiere hasta 8 decisiones más que merezca la pena tomar (secciones prd, arq, tec, ent, cod, dat, sec, cal, git, dep, rsg). Una decisión vale para todo el proyecto y en todo commit: NO sugieras tests de una funcionalidad concreta, ni nada que dependa de la red, ni la historia de dev/main. DEBE (must) solo para lo innegociable; el resto, DEBERÍA (should).\n\
         3) En `why`, una frase: qué del repo te lleva a proponerlo.\n\n=== CAMPOS VACÍOS ===\n"
    );
    for d in defs.iter().filter(|d| d.fixed.is_none()) {
        if items.iter().any(|i| i.field.as_deref() == Some(d.key) && i.status == "confirmed") {
            continue;
        }
        let opts = if d.options.is_empty() { String::new() } else { format!(" (una de: {})", d.options.join(" | ")) };
        p.push_str(&format!("- {} · {} — {}{}\n", d.key, d.label, d.help, opts));
    }
    let decided: Vec<String> = items.iter().filter(|i| i.status == "confirmed").map(|i| format!("- [{}] {}", i.section, i.value.lines().next().unwrap_or(""))).collect();
    if !decided.is_empty() {
        p.push_str(&format!("\n=== YA DECIDIDO (no lo contradigas ni lo repitas) ===\n{}\n", decided.join("\n")));
    }
    let rejected: Vec<String> = items.iter().filter(|i| i.status == "rejected").map(|i| format!("- {}", i.value.lines().next().unwrap_or(""))).collect();
    if !rejected.is_empty() {
        p.push_str(&format!("\n=== DESCARTADO POR EL CREADOR (no lo vuelvas a proponer) ===\n{}\n", rejected.join("\n")));
    }
    p
}

/// The AI's answer, merged into the draft: its earlier undecided proposals
/// go, the new ones come in; what the creator decided or discarded stays.
pub fn merge_proposals(items: &mut Vec<Item>, answer: &Value) {
    let defs = template();
    items.retain(|i| !(i.status == "proposed" && i.by == "ai"));
    let rejected: Vec<String> = items.iter().filter(|i| i.status == "rejected").map(|i| i.value.trim().to_lowercase()).collect();
    for f in answer.get("fields").and_then(Value::as_array).cloned().unwrap_or_default() {
        let key = f.get("key").and_then(Value::as_str).unwrap_or("");
        let value = f.get("value").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let Some(def) = defs.iter().find(|d| d.key == key && d.fixed.is_none()) else { continue };
        if value.is_empty() || rejected.contains(&value.to_lowercase()) || items.iter().any(|i| i.field.as_deref() == Some(key) && i.status == "confirmed") {
            continue;
        }
        if !def.options.is_empty() && !def.options.contains(&value.as_str()) {
            continue;
        }
        items.push(Item {
            id: uuid::Uuid::new_v4().to_string(),
            section: def.section.into(),
            field: Some(key.into()),
            value,
            level: def.level.into(),
            check: None,
            status: "proposed".into(),
            by: "ai".into(),
            why: f.get("why").and_then(Value::as_str).map(str::to_string),
            rule_id: None,
        });
    }
    for s in answer.get("suggestions").and_then(Value::as_array).cloned().unwrap_or_default().into_iter().take(8) {
        let text = s.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let section = s.get("section").and_then(Value::as_str).unwrap_or("").to_string();
        if text.is_empty() || !brain::SECTIONS.iter().any(|(k, _, _)| *k == section) || rejected.contains(&text.to_lowercase()) {
            continue;
        }
        items.push(Item {
            id: uuid::Uuid::new_v4().to_string(),
            section,
            field: None,
            value: text,
            level: if s.get("level").and_then(Value::as_str) == Some("must") { "must".into() } else { "should".into() },
            check: None,
            status: "proposed".into(),
            by: "ai".into(),
            why: s.get("why").and_then(Value::as_str).map(str::to_string),
            rule_id: None,
        });
    }
}

// ---------------------------------------------------------------- commands

#[tauri::command]
pub async fn factory_decisions_get<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<DraftView, String> {
    crate::factory::with_db(app, move |conn| view(conn, &project_id)).await
}

/// Saves the creator's edits: a field typed or a suggestion added by the
/// creator is confirmed by them.
#[tauri::command]
pub async fn factory_decisions_save<R: Runtime>(app: AppHandle<R>, project_id: String, items: Vec<Item>) -> Result<DraftView, String> {
    crate::factory::with_db(app, move |conn| {
        let based = load_draft(conn, &project_id)?.and_then(|(_, b)| b);
        let mut items = items;
        assign_ids(&mut items);
        save_draft(conn, &project_id, &items, based)?;
        view(conn, &project_id)
    })
    .await
}

/// «Partir del brain vigente»: the draft rebuilt from the brain sealed now.
#[tauri::command]
pub async fn factory_decisions_reset<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<DraftView, String> {
    crate::factory::with_db(app, move |conn| {
        let version = current_version(conn, &project_id);
        let items = brain::load(conn, &project_id, None)?.map(|b| from_sealed(&b.rules)).unwrap_or_default();
        save_draft(conn, &project_id, &items, version)?;
        view(conn, &project_id)
    })
    .await
}

/// «Pedir propuestas a la IA»: the spec writer reads the repo and proposes
/// values for the empty fields and a few more decisions.
#[tauri::command]
pub async fn factory_decisions_propose<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<DraftView, String> {
    crate::blocking::run_blocking(move || {
        let root = crate::app_data::root(&app).ok();
        let db = app.state::<DbConnection>();
        let (project, items, account, model, accounts) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            view(&conn, &project_id)?;
            let project = super::store::get(&conn, &project_id)?;
            let items = load_draft(&conn, &project_id)?.map(|(i, _)| i).unwrap_or_default();
            let (account, model) = super::planner::spec_agent(&conn, &project_id)?;
            let accounts = super::agent::accounts_for(&conn, std::slice::from_ref(&account))?;
            (project, items, account, model, accounts)
        };
        let runner = CliRunner::from_accounts(accounts, root)?;
        super::procs::clear_job(&project_id, "decide");
        let call = AgentCall {
            project: project_id.clone(),
            key: "decide".into(),
            account_id: account,
            model,
            cwd: project.project_path.clone().into(),
            prompt: propose_prompt(&project.project_name, &items),
            schema: propose_schema(),
            resume: None,
            mcp_config: None,
            write: false,
            timeout: Duration::from_secs(15 * 60),
        };
        let result = runner.run(&call, &mut |_| {});
        let answer = match result.outcome {
            Outcome::Ok => result.json.ok_or("la IA no devolvió propuestas")?,
            Outcome::Limit { resume_at } => return Err(format!("la cuenta del spec writer llegó a su límite; se renueva a las {}", resume_at.with_timezone(&chrono::Local).format("%H:%M"))),
            Outcome::NoCredits => return Err("la cuenta del spec writer no tiene créditos".into()),
            Outcome::Stopped => return Err("cancelado".into()),
            Outcome::Failed(why) => return Err(format!("la IA falló: {why}")),
        };
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        // Re-read: the creator may have decided things while it thought.
        let (mut now, based) = load_draft(&conn, &project_id)?.unwrap_or_default();
        merge_proposals(&mut now, &answer);
        save_draft(&conn, &project_id, &now, based)?;
        view(&conn, &project_id)
    })
    .await?
}

#[tauri::command]
pub fn factory_decisions_cancel(project_id: String) -> bool {
    super::procs::stop_job(&project_id, "decide") > 0
}

/// «Revisar y sellar»: the confirmed decisions as a brain, for the same
/// report, lint and try as any other way in.
#[tauri::command]
pub async fn factory_decisions_render<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<Report, String> {
    crate::factory::with_db(app, move |conn| {
        let (mut items, based) = load_draft(conn, &project_id)?.ok_or("todavía no hay decisiones")?;
        assign_ids(&mut items);
        save_draft(conn, &project_id, &items, based)?;
        let pending = brain::load(conn, &project_id, None)?.map(|b| brain::parse(&b.markdown).pending).unwrap_or_default();
        let name = super::store::get(conn, &project_id)?.project_name.to_lowercase().replace(' ', "-");
        Ok(brain::parse(&brain::render(&name, None, &render_rules(&items), &pending)))
    })
    .await
}

/// After sealing from the decisions: the draft is now that version.
#[tauri::command]
pub async fn factory_decisions_sealed<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<(), String> {
    crate::factory::with_db(app, move |conn| {
        let version = current_version(conn, &project_id);
        let items = load_draft(conn, &project_id)?.map(|(i, _)| i).unwrap_or_default();
        save_draft(conn, &project_id, &items, version)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirmed(field: &str, value: &str) -> Item {
        let def = template().into_iter().find(|d| d.key == field).unwrap();
        Item { id: field.into(), section: def.section.into(), field: Some(field.into()), value: value.into(), level: def.level.into(), check: None, status: "confirmed".into(), by: "creator".into(), why: None, rule_id: None }
    }

    /// Every check the Factory writes passes its own lint: no syntax error,
    /// no trap, nothing that clashes with how the line works.
    #[test]
    fn every_generated_guard_passes_the_brains_own_lint() {
        let mut items = vec![
            confirmed("tec.gestor", "pnpm"),
            confirmed("tec.versiones", "react@19.1.0\n@types/react@19.1.2"),
            confirmed("tec.prohibidas", "moment\nlodash"),
            confirmed("arq.carpetas", "src/api\nsrc/components"),
            confirmed("arq.imports", "src/components → src/api\nsrc/pages -> src/db"),
            confirmed("arq.no_tocar", "migrations/\nLICENSE"),
            confirmed("ent.runtime", "node 24\npython 3.12\ndocker"),
            confirmed("ent.navegadores", "chromium\nfirefox"),
            confirmed("cod.max_lineas", "400"),
            confirmed("git.commits", "convencionales (feat:, fix:…)"),
            confirmed("cal.suite", "npm run verify"),
            confirmed("rsg.tamano", "600"),
            confirmed("prd.que", "Una Pokédex"),
        ];
        assign_ids(&mut items);
        let rules = render_rules(&items);
        let with_check: Vec<&Rule> = rules.iter().filter(|r| r.check.is_some()).collect();
        assert_eq!(with_check.len(), 13, "{:?}", rules.iter().map(|r| (&r.id, &r.check)).collect::<Vec<_>>());
        let errors: Vec<_> = super::super::lint::lint_static(&rules, "POK-1").into_iter().filter(|f| f.severity == "error").collect();
        assert!(errors.is_empty(), "{errors:?}");
        // The branch rule the Factory writes accepts the branches it makes.
        assert!(super::super::lint::branch_pattern(&rules).unwrap().is_match("feat/POK-1-buscador"));
        assert!(rules.iter().any(|r| r.text == "Gestor de paquetes: pnpm" && r.check.as_deref().unwrap().contains("pnpm-lock.yaml")));
        assert!(rules.iter().any(|r| r.section == "git" && r.text.starts_with("Rama base: dev")), "fixed fields are always there");
    }

    #[test]
    fn generated_guards_do_what_they_say() {
        let dir = std::env::temp_dir().join(format!("hangar-guard-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("src/components")).unwrap();
        std::fs::write(dir.join("package.json"), r#"{"dependencies":{"react":"19.1.0","moment":"2.0.0"}}"#).unwrap();
        std::fs::write(dir.join("src/components/Card.tsx"), "import { get } from '@/api/client'\n").unwrap();
        let run = |c: String| std::process::Command::new("sh").args(["-c", &c]).current_dir(&dir).output().unwrap().status.success();
        assert!(run(guard("tec.versiones", "react@19.1.0", Some("npm")).unwrap()));
        assert!(!run(guard("tec.versiones", "react@19.2.0", Some("npm")).unwrap()));
        assert!(!run(guard("tec.prohibidas", "moment", Some("npm")).unwrap()));
        assert!(run(guard("tec.prohibidas", "lodash", Some("npm")).unwrap()));
        assert!(!run(guard("arq.imports", "src/components → src/api", None).unwrap()), "components imports the api: red");
        assert!(run(guard("arq.imports", "src/components → src/db", None).unwrap()));
        assert!(run(guard("arq.carpetas", "src/components", None).unwrap()));
        assert!(!run(guard("arq.carpetas", "src/hooks", None).unwrap()));
        assert!(!run(guard("tec.gestor", "pnpm", None).unwrap()), "no pnpm-lock.yaml");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_ai_proposes_and_only_the_creator_decides() {
        let mut items = vec![confirmed("tec.gestor", "npm"), Item { status: "rejected".into(), by: "ai".into(), ..confirmed("prd.que", "Una red social") }];
        let answer = json!({
            "fields": [
                {"key":"tec.gestor","value":"pnpm","why":"x"},
                {"key":"prd.que","value":"Una red social","why":"x"},
                {"key":"tec.lenguaje","value":"TypeScript 5","why":"tsconfig.json"},
                {"key":"git.base","value":"main","why":"x"},
                {"key":"cod.idioma_nombres","value":"klingon","why":"x"}
            ],
            "suggestions": [{"section":"sec","text":"Ningún secreto en el repo","level":"must","why":"x"}]
        });
        merge_proposals(&mut items, &answer);
        let proposed: Vec<&Item> = items.iter().filter(|i| i.status == "proposed").collect();
        assert_eq!(proposed.len(), 2, "{proposed:?}");
        assert!(proposed.iter().any(|i| i.field.as_deref() == Some("tec.lenguaje")));
        assert!(proposed.iter().any(|i| i.field.is_none() && i.value == "Ningún secreto en el repo"));
        assert!(!items.iter().any(|i| i.field.as_deref() == Some("tec.gestor") && i.value == "pnpm"), "a decided field isn't overridden");
        assert!(render_rules(&items).iter().all(|r| !r.text.contains("TypeScript") && !r.text.contains("secreto")), "proposals aren't in the brain until confirmed");
        // A second ask replaces the AI's undecided proposals, never the creator's.
        merge_proposals(&mut items, &json!({"fields": [], "suggestions": []}));
        assert_eq!(items.iter().filter(|i| i.status == "proposed").count(), 0);
        assert_eq!(items.iter().filter(|i| i.status == "rejected").count(), 1);
    }

    #[test]
    fn a_rule_keeps_its_id_across_edits() {
        let mut items = vec![confirmed("tec.lenguaje", "TS"), confirmed("tec.framework", "React")];
        assign_ids(&mut items);
        let ids = |items: &[Item], f: &str| items.iter().find(|i| i.field.as_deref() == Some(f)).and_then(|i| i.rule_id.clone());
        assert_eq!((ids(&items, "tec.lenguaje").as_deref(), ids(&items, "tec.framework").as_deref()), (Some("TEC-01"), Some("TEC-02")));
        assert_eq!(ids(&items, "git.base").as_deref(), Some("GIT-01"), "the Factory's own fields get a real id too");
        items.retain(|i| i.field.as_deref() != Some("tec.lenguaje"));
        items.push(confirmed("tec.gestor", "npm"));
        assign_ids(&mut items);
        assert_eq!(ids(&items, "tec.framework").as_deref(), Some("TEC-02"), "kept");
        assert_eq!(ids(&items, "tec.gestor").as_deref(), Some("TEC-01"), "the free one");
        let rules = render_rules(&items);
        assert!(brain::parse(&brain::render("x", None, &rules, &[])).assigned.is_empty(), "every id is one the parser reads");
        let sealed = vec![Rule { id: "ARQ-07".into(), section: "arq".into(), level: "should".into(), text: "x".into(), check: None }];
        let imported = from_sealed(&sealed);
        assert_eq!((imported[0].rule_id.as_deref(), imported[0].status.as_str(), imported[0].by.as_str()), (Some("ARQ-07"), "confirmed", "brain"));
    }
}
