//! The brain: the project's rules, defined once in a conversation with the AI
//! and sealed as `BRAIN.md vN`. Every order freezes the version it started
//! with, and every decision on the line cites one of its rules.
//!
//! There is one way in — import. The terminal session, the guide pasted into
//! another conversation and a file someone already has all end as markdown
//! that goes through `parse`, gets a report, and is sealed from there.

use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// The sections, from the start of a project to production, in the order the
/// file lists them.
pub const SECTIONS: [(&str, &str, &str); 11] = [
    ("prd", "Producto", "PRD"),
    ("arq", "Arquitectura", "ARQ"),
    ("tec", "Tecnologías", "TEC"),
    ("cod", "Código", "COD"),
    ("dat", "Datos", "DAT"),
    ("sec", "Seguridad", "SEC"),
    ("cal", "Calidad", "CAL"),
    ("git", "Git y entrega", "GIT"),
    ("dep", "Despliegue", "DEP"),
    ("rsg", "Riesgo", "RSG"),
    ("dec", "Decisiones", "DEC"),
];

/// Without these there is nothing for the line to decide with, so a brain
/// missing any of them can't be sealed.
pub const REQUIRED: [&str; 7] = ["prd", "arq", "tec", "cod", "cal", "git", "rsg"];

/// Heading prefixes (accents folded, lower case) and the section they open.
const KEYS: [(&str, &str); 13] = [
    ("producto", "prd"),
    ("arquitectura", "arq"),
    ("tecnolog", "tec"),
    ("stack", "tec"),
    ("codigo", "cod"),
    ("datos", "dat"),
    ("seguridad", "sec"),
    ("calidad", "cal"),
    ("git", "git"),
    ("despliegue", "dep"),
    ("riesgo", "rsg"),
    ("decision", "dec"),
    ("pendiente", "pend"),
];

pub fn section_name(key: &str) -> &'static str {
    SECTIONS.iter().find(|(k, _, _)| *k == key).map(|(_, n, _)| *n).unwrap_or("")
}

fn prefix(key: &str) -> &'static str {
    SECTIONS.iter().find(|(k, _, _)| *k == key).map(|(_, _, p)| *p).unwrap_or("REG")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    pub section: String,
    /// `must` (DEBE) or `should` (DEBERÍA).
    pub level: String,
    pub text: String,
    /// A command that checks it without AI, run by the test gate.
    pub check: Option<String>,
}

/// What an import found, as the wizard shows it before sealing.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub rules: Vec<Rule>,
    /// `## Pendiente`: questions the conversation left open.
    pub pending: Vec<String>,
    /// IDs Hangar gave to rules that came without one.
    pub assigned: Vec<String>,
    /// Headings that aren't one of the sections, ignored.
    pub unknown: Vec<String>,
    pub sections: Vec<String>,
    pub missing: Vec<String>,
    pub missing_required: Vec<String>,
    pub checks: usize,
    /// The file in its sealed shape: every rule with its ID, sections in order.
    pub markdown: String,
}

/// `Código` → `codigo`: what the heading match compares.
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'á' | 'à' | 'ä' | 'â' => 'a',
            'é' | 'è' | 'ë' | 'ê' => 'e',
            'í' | 'ì' | 'ï' | 'î' => 'i',
            'ó' | 'ò' | 'ö' | 'ô' => 'o',
            'ú' | 'ù' | 'ü' | 'û' => 'u',
            'ñ' => 'n',
            other => other,
        })
        .collect()
}

/// A leading `ARQ-03` and its separator.
fn take_id(body: &str) -> Option<(String, &str)> {
    let re = regex::Regex::new(r"^([A-Z]{2,4}-\d+)\s*[·:—-]?\s*").ok()?;
    let m = re.captures(body)?;
    let all = m.get(0)?;
    Some((m.get(1)?.as_str().to_string(), &body[all.end()..]))
}

/// A leading `DEBE` / `DEBERÍA` (or MUST / SHOULD) and its separator.
fn take_level(body: &str) -> Option<(&'static str, &str)> {
    let re = regex::Regex::new(r"(?i)^(DEBERÍA|DEBERIA|DEBE|SHOULD|MUST)\s*[·:—-]?\s*").ok()?;
    let m = re.captures(body)?;
    let word = m.get(1)?.as_str().to_uppercase();
    let level = if word.starts_with("DEBER") || word == "SHOULD" { "should" } else { "must" };
    Some((level, &body[m.get(0)?.end()..]))
}

/// The command out of what follows `check:` — `pnpm test`, or the first
/// backticked span of «`pnpm test` (`vitest run --coverage` con thresholds)».
fn check_command(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let cmd = match raw.strip_prefix('`') {
        Some(rest) => rest.split('`').next().unwrap_or(""),
        None => raw,
    };
    Some(cmd.trim().to_string()).filter(|c| !c.is_empty())
}

/// Reads a BRAIN.md — or a CLAUDE.md / AGENTS.md written the same way — and
/// says what it found. Never fails: whatever isn't recognised is reported.
pub fn parse(text: &str) -> Report {
    let mut rules: Vec<Rule> = Vec::new();
    let mut pending = Vec::new();
    let mut unknown = Vec::new();
    let mut unnamed: Vec<usize> = Vec::new();
    let mut sec: Option<&str> = None;
    let heading = regex::Regex::new(r"^#{2,3}\s+(.+)$").expect("static regex");
    let inline = regex::Regex::new(r"(?i)\s+check:\s*(.+)$").expect("static regex");

    for raw in text.lines() {
        let line = raw.trim();
        if let Some(h) = heading.captures(line) {
            let title = h[1].trim();
            let n = fold(title);
            sec = KEYS.iter().find(|(k, _)| n.starts_with(k)).map(|(_, s)| *s);
            if sec.is_none() {
                unknown.push(title.split('(').next().unwrap_or(title).trim().to_string());
            }
            continue;
        }
        let Some(current) = sec else { continue };
        if let Some(rest) = line.strip_prefix("check:").or_else(|| line.strip_prefix("Check:")) {
            if current != "pend" {
                if let Some(last) = rules.last_mut() {
                    last.check = check_command(rest);
                }
            }
            continue;
        }
        let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) else { continue };
        let item = item.trim();
        if current == "pend" {
            pending.push(item.to_string());
            continue;
        }
        let (id, body) = match take_id(item) {
            Some((id, rest)) => (Some(id), rest),
            None => (None, item),
        };
        let (level, body) = take_level(body).unwrap_or(("must", body));
        if id.is_none() {
            unnamed.push(rules.len());
        }
        // `check:` on the rule's own line, as many hand-written brains put it.
        let (text, check) = match inline.captures(body) {
            Some(m) => (body[..m.get(0).expect("whole match").start()].trim(), check_command(&m[1])),
            None => (body.trim(), None),
        };
        rules.push(Rule { id: id.unwrap_or_default(), section: current.to_string(), level: level.to_string(), text: text.to_string(), check });
    }

    let mut used: std::collections::HashSet<String> = rules.iter().map(|r| r.id.clone()).filter(|id| !id.is_empty()).collect();
    let mut assigned = Vec::new();
    for i in unnamed {
        let p = prefix(&rules[i].section);
        let mut n = 1;
        while used.contains(&format!("{p}-{n:02}")) {
            n += 1;
        }
        let id = format!("{p}-{n:02}");
        used.insert(id.clone());
        assigned.push(id.clone());
        rules[i].id = id;
    }

    let found: Vec<String> = SECTIONS.iter().filter(|(k, _, _)| rules.iter().any(|r| r.section == *k)).map(|(k, _, _)| k.to_string()).collect();
    let missing = SECTIONS.iter().filter(|(k, _, _)| !found.iter().any(|f| f == k)).map(|(k, _, _)| k.to_string()).collect();
    let missing_required = REQUIRED.iter().filter(|k| !found.iter().any(|f| f == *k)).map(|k| k.to_string()).collect();
    let checks = rules.iter().filter(|r| r.check.is_some()).count();
    let project = frontmatter(text, "proyecto");
    let markdown = render(project.as_deref().unwrap_or("proyecto"), None, &rules, &pending);
    Report { rules, pending, assigned, unknown, sections: found, missing, missing_required, checks, markdown }
}

/// `proyecto: citas-vet` out of the front matter.
fn frontmatter(text: &str, key: &str) -> Option<String> {
    let mut lines = text.lines().map(str::trim);
    if lines.next()? != "---" {
        return None;
    }
    for line in lines {
        if line == "---" {
            return None;
        }
        if let Some((k, v)) = line.split_once(':') {
            if fold(k.trim()) == key {
                return Some(v.trim().to_string()).filter(|v| !v.is_empty());
            }
        }
    }
    None
}

/// The brain in its one shape: front matter, the sections in order, one rule
/// per line with its `check:` under it, and what's still open.
pub fn render(project: &str, version: Option<i64>, rules: &[Rule], pending: &[String]) -> String {
    let mut out = vec!["---".to_string(), format!("proyecto: {project}")];
    out.push(match version {
        Some(v) => format!("versión: {v}"),
        None => "versión: borrador".to_string(),
    });
    out.push("---".to_string());
    for (key, name, _) in SECTIONS {
        let in_section: Vec<&Rule> = rules.iter().filter(|r| r.section == key).collect();
        if in_section.is_empty() {
            continue;
        }
        out.push(String::new());
        out.push(format!("## {name}"));
        for r in in_section {
            out.push(format!("- {} · {} · {}", r.id, if r.level == "should" { "DEBERÍA" } else { "DEBE" }, r.text));
            if let Some(c) = &r.check {
                out.push(format!("  check: {c}"));
            }
        }
    }
    if !pending.is_empty() {
        out.push(String::new());
        out.push("## Pendiente".to_string());
        out.extend(pending.iter().map(|p| format!("- {p}")));
    }
    out.join("\n") + "\n"
}

/// What to paste at the end of a conversation with any AI so it answers with
/// a BRAIN.md this parser reads.
pub const GUIDE: &str = "Con todo lo que hemos hablado de este proyecto, escribe su BRAIN.md: las reglas con las que una IA va a construirlo sin preguntarme. Devuelve SOLO el archivo, con este formato exacto:

---
proyecto: <nombre-en-minúsculas>
versión: 1
---

## Producto        (problema, usuarios, alcance y lo que queda fuera, glosario)
## Arquitectura    (módulos, capas, patrones obligatorios, lo que no se toca)
## Tecnologías     (stack y versiones, librerías permitidas y prohibidas)
## Código          (estilo, nombres, errores, cómo se compila y se prueba)
## Datos           (modelo, migraciones, compatibilidad)
## Seguridad       (secretos, autenticación, datos sensibles)
## Calidad         (tests, comandos de lint/test, qué significa «hecho»)
## Git y entrega   (nombre de rama, rama base, commits, MR)
## Despliegue      (entornos, CI/CD, observabilidad, rollback)
## Riesgo          (qué obliga a parar y preguntar; tamaño máximo de una tarea)
## Decisiones      (decisión — fecha — porqué)
## Pendiente       (preguntas que no se resolvieron)

Cada regla en una línea:
- <ID> · DEBE|DEBERÍA · <regla concreta y comprobable>
  check: <comando que la comprueba>   (solo si existe)

IDs con el prefijo de su sección y número: PRD-01, ARQ-01, TEC-01, COD-01, DAT-01, SEC-01, CAL-01, GIT-01, DEP-01, RSG-01, DEC-01.
Nada vago («código limpio»): si no se puede comprobar, reescríbelo hasta que se pueda.
No inventes: lo que no hablamos va a «## Pendiente» como pregunta.";

/// The prototype's Citas Vet brain, offered as «usar el ejemplo».
pub const EXAMPLE: &str = include_str!("examples/citas-vet-brain.md");

/// Where the terminal session leaves the file for Hangar to import.
pub const GENERATED: &str = ".hangar/brain/BRAIN.md";

/// The first thing the definition session is told: what it's for, and that
/// it writes nothing until asked.
pub fn session_prompt(project: &str, base: Option<&str>, inbox: &[String]) -> String {
    let mut p = format!(
        "Vamos a definir el brain de «{project}»: las reglas con las que una IA lo va a construir de principio a producción sin preguntarme. \
Lee el repo para no preguntar lo que ya se ve en él. Después recorre conmigo, una a una, estas secciones: \
Producto, Arquitectura, Tecnologías, Código, Datos, Seguridad, Calidad, Git y entrega, Despliegue, Riesgo y Decisiones. \
Haz una o dos preguntas cada vez y propón reglas concretas y comprobables, con su comando check: cuando exista. \
No escribas ningún archivo hasta que te pida generar el BRAIN.md."
    );
    if let Some(base) = base {
        p.push_str(&format!("\n\nPartimos del brain sellado actual; cámbialo solo donde haga falta:\n\n{base}"));
    }
    if !inbox.is_empty() {
        p.push_str("\n\nLa bandeja de enmiendas trae estas preguntas que el brain no cubría; resuélvelas conmigo:\n");
        for q in inbox {
            p.push_str(&format!("- {q}\n"));
        }
    }
    p
}

/// What «Generar BRAIN.md» types into the session, as one line.
pub fn generate_instruction() -> String {
    format!("Ahora genera el BRAIN.md con todo lo que hemos hablado y guárdalo en {GENERATED} (crea la carpeta si no existe). {}", GUIDE.replace('\n', " "))
}

// ---------------------------------------------------------------- storage

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub version: i64,
    pub path: String,
    pub sha: String,
    pub rule_count: i64,
    pub source: String,
    pub sealed_at: String,
}

/// A sealed version with its rules, as the line and the Brain tab read it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sealed {
    #[serde(flatten)]
    pub version: Version,
    pub rules: Vec<Rule>,
    pub markdown: String,
}

fn sha(text: &str) -> String {
    // FNV-1a: enough to tell two sealed files apart at a glance.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

pub fn brain_dir(root: &Path, project_id: &str) -> PathBuf {
    root.join("factory").join(project_id).join("brain")
}

/// Seals `markdown` as the project's next version: the file is written once
/// and never changed, its rules are indexed, and the project points at it.
pub fn seal(conn: &Connection, root: &Path, project_id: &str, project_name: &str, markdown: &str, source: &str) -> Result<Sealed, String> {
    let report = parse(markdown);
    if !report.missing_required.is_empty() {
        let names: Vec<&str> = report.missing_required.iter().map(|k| section_name(k)).collect();
        return Err(format!("faltan secciones obligatorias para sellar: {}", names.join(", ")));
    }
    let next: i64 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) + 1 FROM factory_brain_versions WHERE project_id = ?1", params![project_id], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let slug = frontmatter(markdown, "proyecto").unwrap_or_else(|| project_name.to_lowercase().replace(' ', "-"));
    let text = render(&slug, Some(next), &report.rules, &report.pending);
    let dir = brain_dir(root, project_id);
    std::fs::create_dir_all(&dir).map_err(|e| format!("no se pudo crear {}: {e}", dir.display()))?;
    let path = dir.join(format!("v{next}.md"));
    std::fs::write(&path, &text).map_err(|e| format!("no se pudo escribir {}: {e}", path.display()))?;

    let now = Utc::now().to_rfc3339();
    let version = Version { version: next, path: path.to_string_lossy().to_string(), sha: sha(&text), rule_count: report.rules.len() as i64, source: source.to_string(), sealed_at: now };
    conn.execute(
        "INSERT INTO factory_brain_versions (project_id, version, path, sha, rule_count, source, sealed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![project_id, version.version, version.path, version.sha, version.rule_count, version.source, version.sealed_at],
    )
    .map_err(|e| e.to_string())?;
    for (i, r) in report.rules.iter().enumerate() {
        conn.execute(
            "INSERT INTO factory_brain_rules (project_id, version, rule_id, section, level, text, check_cmd, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![project_id, next, r.id, r.section, r.level, r.text, r.check, i as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    conn.execute("UPDATE factory_projects SET brain_version = ?2, updated_at = ?3 WHERE project_id = ?1", params![project_id, next, Utc::now().to_rfc3339()])
        .map_err(|e| e.to_string())?;
    // Questions this version answered leave the inbox.
    conn.execute("UPDATE factory_amendments SET status = 'included', included_in = ?2 WHERE project_id = ?1 AND status = 'open' AND ?2 > 1", params![project_id, next])
        .map_err(|e| e.to_string())?;
    Ok(Sealed { version, rules: report.rules, markdown: text })
}

/// Rebuilds the rule index of a sealed version from its file, with this
/// parser. The file never changes; the index is derived from it, so a brain
/// sealed by an older parser (that missed `check:` written on the rule's own
/// line) gets its checks back. Returns how many rules have a check now.
pub fn reindex(conn: &Connection, project_id: &str, version: i64) -> Result<usize, String> {
    let Some(meta) = versions(conn, project_id)?.into_iter().find(|v| v.version == version) else { return Ok(0) };
    let text = std::fs::read_to_string(&meta.path).map_err(|e| format!("no se pudo leer {}: {e}", meta.path))?;
    let report = parse(&text);
    conn.execute("DELETE FROM factory_brain_rules WHERE project_id = ?1 AND version = ?2", params![project_id, version]).map_err(|e| e.to_string())?;
    for (i, r) in report.rules.iter().enumerate() {
        conn.execute(
            "INSERT INTO factory_brain_rules (project_id, version, rule_id, section, level, text, check_cmd, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![project_id, version, r.id, r.section, r.level, r.text, r.check, i as i64],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(report.checks)
}

pub fn versions(conn: &Connection, project_id: &str) -> Result<Vec<Version>, String> {
    let mut stmt = conn
        .prepare("SELECT version, path, sha, rule_count, source, sealed_at FROM factory_brain_versions WHERE project_id = ?1 ORDER BY version DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], |r| Ok(Version { version: r.get(0)?, path: r.get(1)?, sha: r.get(2)?, rule_count: r.get(3)?, source: r.get(4)?, sealed_at: r.get(5)? }))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// One version with its rules; `None` for the one the project uses now.
pub fn load(conn: &Connection, project_id: &str, version: Option<i64>) -> Result<Option<Sealed>, String> {
    let wanted: Option<i64> = match version {
        Some(v) => Some(v),
        None => conn
            .query_row("SELECT brain_version FROM factory_projects WHERE project_id = ?1", params![project_id], |r| r.get(0))
            .optional()
            .map_err(|e| e.to_string())?
            .flatten(),
    };
    let Some(v) = wanted else { return Ok(None) };
    let Some(meta) = versions(conn, project_id)?.into_iter().find(|x| x.version == v) else { return Ok(None) };
    let mut stmt = conn
        .prepare("SELECT rule_id, section, level, text, check_cmd FROM factory_brain_rules WHERE project_id = ?1 AND version = ?2 ORDER BY position")
        .map_err(|e| e.to_string())?;
    let rules = stmt
        .query_map(params![project_id, v], |r| Ok(Rule { id: r.get(0)?, section: r.get(1)?, level: r.get(2)?, text: r.get(3)?, check: r.get(4)? }))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let markdown = std::fs::read_to_string(&meta.path).unwrap_or_default();
    Ok(Some(Sealed { version: meta, rules, markdown }))
}

/// The rules of the given sections only — what one agent on the line reads.
pub fn excerpt(sealed: &Sealed, sections: &[String]) -> String {
    let wanted: Vec<Rule> = sealed.rules.iter().filter(|r| sections.is_empty() || sections.contains(&r.section)).cloned().collect();
    render("brain", Some(sealed.version.version), &wanted, &[])
}

/// The sealed file in the shape another tool reads: BRAIN.md as is, or with
/// a preface for AGENTS.md / CLAUDE.md that says how to use it.
pub fn export(sealed: &Sealed, format: &str, project_name: &str) -> Result<String, String> {
    let body = &sealed.markdown;
    match format {
        "brain" => Ok(body.clone()),
        "agents" | "claude" => Ok(format!(
            "# {project_name} — reglas del proyecto\n\n\
Generado por Hangar Factory desde BRAIN.md v{}. Es la fuente de la verdad: toda decisión cita el ID de la regla que la justifica \
(p. ej. «ARQ-02»). Lo que estas reglas no cubren no se decide: se pregunta.\n\n{body}",
            sealed.version.version
        )),
        other => Err(format!("formato desconocido `{other}`")),
    }
}

// ---------------------------------------------------------------- amendments

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Amendment {
    pub id: String,
    pub question: String,
    pub order_key: Option<String>,
    /// What the line did meanwhile — always the conservative choice.
    pub provisional: Option<String>,
    /// `open`, `included` (a later version answered it) or `dismissed`.
    pub status: String,
    pub included_in: Option<i64>,
    pub created_at: String,
}

pub fn add_amendment(conn: &Connection, project_id: &str, question: &str, order_key: Option<&str>, provisional: Option<&str>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO factory_amendments (id, project_id, question, order_key, provisional, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6)",
        params![uuid::Uuid::new_v4().to_string(), project_id, question, order_key, provisional, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn amendments(conn: &Connection, project_id: &str) -> Result<Vec<Amendment>, String> {
    let mut stmt = conn
        .prepare("SELECT id, question, order_key, provisional, status, included_in, created_at FROM factory_amendments WHERE project_id = ?1 ORDER BY created_at DESC")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], |r| {
            Ok(Amendment { id: r.get(0)?, question: r.get(1)?, order_key: r.get(2)?, provisional: r.get(3)?, status: r.get(4)?, included_in: r.get(5)?, created_at: r.get(6)? })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

pub fn set_amendment_status(conn: &Connection, id: &str, status: &str) -> Result<(), String> {
    if !["open", "dismissed"].contains(&status) {
        return Err(format!("estado desconocido `{status}`"));
    }
    conn.execute("UPDATE factory_amendments SET status = ?2 WHERE id = ?1", params![id, status]).map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------- commands

use tauri::{AppHandle, Manager, Runtime, State};

use crate::db::DbConnection;

#[tauri::command]
pub fn factory_brain_parse(markdown: String) -> Report {
    parse(&markdown)
}

#[tauri::command]
pub fn factory_brain_guide() -> &'static str {
    GUIDE
}

#[tauri::command]
pub fn factory_brain_example() -> &'static str {
    EXAMPLE
}

/// A .md the user picked, read for import. Capped: a brain is a page of rules,
/// not a book.
#[tauri::command]
pub fn factory_read_markdown(path: String) -> Result<String, String> {
    let meta = std::fs::metadata(&path).map_err(|e| format!("no se pudo leer {path}: {e}"))?;
    if meta.len() > 1_000_000 {
        return Err("el archivo pasa de 1 MB: no parece un BRAIN.md".to_string());
    }
    std::fs::read_to_string(&path).map_err(|e| format!("no se pudo leer {path}: {e}"))
}

#[tauri::command]
pub fn factory_brain_seal<R: Runtime>(app: AppHandle<R>, project_id: String, markdown: String, source: String) -> Result<Sealed, String> {
    let root = crate::app_data::root(&app)?;
    let db = app.state::<DbConnection>();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let project = crate::factory::store::get(&conn, &project_id)?;
    seal(&conn, &root, &project_id, &project.project_name, &markdown, &source)
}

#[tauri::command]
pub fn factory_brain_get(db: State<DbConnection>, project_id: String, version: Option<i64>) -> Result<Option<Sealed>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    load(&conn, &project_id, version)
}

#[tauri::command]
pub fn factory_brain_versions(db: State<DbConnection>, project_id: String) -> Result<Vec<Version>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    versions(&conn, &project_id)
}

/// Writes the current version to `path` as BRAIN.md, AGENTS.md or CLAUDE.md.
#[tauri::command]
pub fn factory_brain_export(db: State<DbConnection>, project_id: String, format: String, path: String) -> Result<(), String> {
    let (sealed, name) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let name = crate::factory::store::get(&conn, &project_id)?.project_name;
        (load(&conn, &project_id, None)?.ok_or("todavía no hay un brain sellado")?, name)
    };
    let text = export(&sealed, &format, &name)?;
    std::fs::write(&path, text).map_err(|e| format!("no se pudo escribir {path}: {e}"))
}

/// Opens the definition session: a real Hangar terminal in the project, with
/// the CLI already told what this conversation is for. `base` is a draft to
/// complete (an import with sections missing); `amend` starts from the sealed
/// version and the inbox, for vN+1.
#[tauri::command]
pub async fn factory_brain_session<R: Runtime>(
    app: AppHandle<R>,
    project_id: String,
    account_id: String,
    base: Option<String>,
    amend: bool,
) -> Result<crate::terminals::TerminalInfo, String> {
    crate::blocking::run_blocking(move || {
        let (project, base, inbox) = {
            let db = app.state::<DbConnection>();
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let project = crate::factory::store::get(&conn, &project_id)?;
            let base = match base {
                Some(draft) => Some(draft),
                None if amend => load(&conn, &project_id, None)?.map(|s| s.markdown),
                None => None,
            };
            let inbox: Vec<String> = if amend { amendments(&conn, &project_id)?.into_iter().filter(|a| a.status == "open").map(|a| a.question).collect() } else { Vec::new() };
            (project, base, inbox)
        };
        // A file left by an earlier session would be imported as this one's.
        let _ = std::fs::remove_file(Path::new(&project.project_path).join(GENERATED));
        crate::task_sources::exclude_hangar_dir_locally(Path::new(&project.project_path));
        let prompt = session_prompt(&project.project_name, base.as_deref(), &inbox);
        crate::terminals::open_terminal_blocking(
            app,
            crate::terminals::TerminalTarget { kind: crate::terminals::TargetKind::Project, id: project_id },
            Some(account_id),
            Some(crate::terminals::InitCommand::Start),
            None,
            Some(prompt),
            100,
            30,
        )
    })
    .await?
}

/// «Generar BRAIN.md»: asks the session to write the file where Hangar looks.
#[tauri::command]
pub fn factory_brain_generate<R: Runtime>(app: AppHandle<R>, terminal_id: String) -> Result<(), String> {
    crate::terminals::send_line_to_terminal(&app, &terminal_id, &generate_instruction())
}

/// The file the session wrote, once it's there.
#[tauri::command]
pub fn factory_brain_read_generated(db: State<DbConnection>, project_id: String) -> Result<Option<String>, String> {
    let path = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        Path::new(&crate::factory::store::get(&conn, &project_id)?.project_path).join(GENERATED)
    };
    Ok(std::fs::read_to_string(path).ok().filter(|t| !t.trim().is_empty()))
}

#[tauri::command]
pub fn factory_amendments_list(db: State<DbConnection>, project_id: String) -> Result<Vec<Amendment>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    amendments(&conn, &project_id)
}

#[tauri::command]
pub fn factory_amendment_set_status(db: State<DbConnection>, id: String, status: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_amendment_status(&conn, &id, &status)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The prototype's example: the definition session's 21 rules, two more
    /// without an ID, and one open question.
    pub(crate) const EXAMPLE: &str = super::EXAMPLE;

    #[test]
    fn the_example_gives_23_rules_2_assigned_ids_and_1_pending() {
        let r = parse(EXAMPLE);
        assert_eq!(r.rules.len(), 23);
        assert_eq!(r.assigned, vec!["COD-02", "DEP-02"]);
        assert_eq!(r.pending, vec!["¿Zona horaria de las franjas: la de la clínica o UTC?"]);
        assert_eq!(r.sections.len(), 11);
        assert!(r.missing.is_empty() && r.missing_required.is_empty());
        assert_eq!(r.checks, 7);
        let arq2 = r.rules.iter().find(|x| x.id == "ARQ-02").unwrap();
        assert_eq!(arq2.check.as_deref(), Some("! grep -rn \"session.query\" app/routers"));
        assert_eq!(r.rules.iter().filter(|x| x.level == "should").count(), 4);
    }

    #[test]
    fn headings_are_matched_without_accents_and_the_rest_is_reported() {
        let r = parse("## Codigo\n- MUST · Sin warnings.\n## Misc (cosas)\n- algo\n### Tecnologías\n- SHOULD: Rust 1.80\n");
        assert_eq!(r.rules.iter().map(|x| (x.id.as_str(), x.level.as_str())).collect::<Vec<_>>(), vec![("COD-01", "must"), ("TEC-01", "should")]);
        assert_eq!(r.unknown, vec!["Misc"]);
        assert!(r.missing_required.contains(&"prd".to_string()));
    }

    #[test]
    fn a_check_on_the_rules_own_line_in_backticks_is_read() {
        let r = parse(
            "## Tecnologías\n- TEC-01 · DEBE · TypeScript con `\"strict\": true`.  check: `pnpm tsc --noEmit`\n\
## Calidad\n- CAL-01 · DEBE · Cobertura ≥ 80 %.  check: `pnpm test` (`vitest run --coverage` con `thresholds`)\n\
- CAL-02 · DEBE · e2e.  check: `pnpm e2e --list | grep -q '\\[firefox\\]' && pnpm e2e`\n- CAL-09 · DEBE · Sin check.\n",
        );
        let checks: Vec<(&str, Option<&str>)> = r.rules.iter().map(|x| (x.text.as_str(), x.check.as_deref())).collect();
        assert_eq!(
            checks,
            vec![
                ("TypeScript con `\"strict\": true`.", Some("pnpm tsc --noEmit")),
                ("Cobertura ≥ 80 %.", Some("pnpm test")),
                ("e2e.", Some("pnpm e2e --list | grep -q '\\[firefox\\]' && pnpm e2e")),
                ("Sin check.", None),
            ]
        );
        assert_eq!(parse("## Código\n- COD-01 · DEBE · x\n  check: `ruff check .`\n").rules[0].check.as_deref(), Some("ruff check ."));
    }

    #[test]
    fn the_sealed_shape_parses_back_to_the_same_rules() {
        let r = parse(EXAMPLE);
        let again = parse(&r.markdown);
        assert_eq!(again.rules, r.rules);
        assert!(again.assigned.is_empty(), "every rule already has its ID");
        assert_eq!(frontmatter(&r.markdown, "proyecto").as_deref(), Some("citas-vet"));
    }

    pub(crate) fn conn() -> Connection {
        let conn = crate::factory::store::tests::conn();
        crate::factory::store::add(&conn, "p1", "local").unwrap();
        conn
    }

    #[test]
    fn sealing_writes_an_immutable_file_and_indexes_the_rules() {
        let c = conn();
        let root = std::env::temp_dir().join(format!("hangar-brain-{}", uuid::Uuid::new_v4().simple()));
        let v1 = seal(&c, &root, "p1", "Citas Vet", EXAMPLE, "import").unwrap();
        assert_eq!((v1.version.version, v1.version.rule_count), (1, 23));
        assert!(Path::new(&v1.version.path).ends_with("brain/v1.md"));
        assert!(v1.markdown.contains("versión: 1"));
        assert_eq!(crate::factory::store::get(&c, "p1").unwrap().brain_version, Some(1));

        add_amendment(&c, "p1", "¿Avisar al veterinario al cancelar?", Some("VET-2"), Some("no")).unwrap();
        let v2 = seal(&c, &root, "p1", "Citas Vet", &v1.markdown, "amendment").unwrap();
        assert_eq!(v2.version.version, 2);
        assert_eq!(std::fs::read_to_string(&v1.version.path).unwrap(), v1.markdown, "v1 stays as it was");
        assert_eq!(amendments(&c, "p1").unwrap()[0].status, "included");
        let current = load(&c, "p1", None).unwrap().unwrap();
        assert_eq!((current.version.version, current.rules.len()), (2, 23));
        assert_eq!(load(&c, "p1", Some(1)).unwrap().unwrap().version.version, 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_brain_without_the_required_sections_is_not_sealed() {
        let c = conn();
        let root = std::env::temp_dir().join("hangar-brain-refused");
        let err = seal(&c, &root, "p1", "x", "## Producto\n- DEBE · algo\n", "import").unwrap_err();
        assert!(err.contains("Arquitectura"));
        assert!(versions(&c, "p1").unwrap().is_empty());
    }

    #[test]
    fn it_exports_to_the_three_formats_and_hands_each_agent_its_sections() {
        let c = conn();
        let root = std::env::temp_dir().join(format!("hangar-brain-{}", uuid::Uuid::new_v4().simple()));
        let s = seal(&c, &root, "p1", "Citas Vet", EXAMPLE, "import").unwrap();
        assert_eq!(export(&s, "brain", "Citas Vet").unwrap(), s.markdown);
        for f in ["agents", "claude"] {
            let out = export(&s, f, "Citas Vet").unwrap();
            assert!(out.starts_with("# Citas Vet") && out.contains("ARQ-02") && out.contains("v1"));
        }
        assert!(export(&s, "pdf", "x").is_err());
        let sec = excerpt(&s, &["sec".to_string()]);
        assert!(sec.contains("SEC-01") && !sec.contains("ARQ-01"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
