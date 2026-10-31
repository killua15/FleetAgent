//! The line: one order from its spec to its delivery.
//!
//! Ingeniería (spec → critic → ✋ by mode) → Fabricación (builder, in the
//! order's own worktree) → Testers (they write the tests and commit) → the
//! test gate (no AI, never skipped) → Revisión (code, security and — when the
//! order touches the interface — UI then UX with Chrome) → Verifier →
//! Approver (only ever sees green) → Entrega (MR, or a local merge).
//!
//! Every stage is persisted before and after it runs, so a usage limit, a ⏹
//! or Hangar closing leaves the order exactly where it was, in the same
//! conversation, to go on later.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::agent::{AgentCall, Outcome, Runner};
use super::brain::{self, Sealed};
use super::orders::{self, Order};
use super::team::Agent;
use crate::db::DbConnection;

/// Rounds in red (or with confirmed findings) before the builder escalates.
pub const K_ROUNDS: i64 = 2;
/// Rounds after which the order fails instead of going round again.
pub const MAX_ROUNDS: i64 = 4;

const AGENT_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const GATE_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// The station a stage belongs to, as the live view groups them.
pub fn station(stage: &str) -> &'static str {
    match stage {
        "spec" | "critic" | "spec_review" => "ing",
        "build" => "fab",
        "test" | "gate" => "test",
        "review" => "rev",
        "verify" | "approve" => "dec",
        _ => "ent",
    }
}

/// What a run keeps between stages.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RunState {
    pub spec: Option<Value>,
    /// Agent id → its conversation, to resume after a limit or another round.
    pub sessions: HashMap<String, String>,
    /// Agent id → the account it uses instead («Usar otra cuenta»).
    pub accounts: HashMap<String, String>,
    /// What the builder has to fix: the gate's failures or confirmed findings.
    pub feedback: Option<String>,
    pub findings: Vec<Value>,
    pub warnings: Vec<String>,
    pub escalated: bool,
    /// Whether the last gate run was green. The approver needs it.
    pub gate_ok: bool,
    pub gate: Vec<GateResult>,
    /// Why it waits for a person: `spec_review`, `diff`, `approver`, `credits`, `merge`.
    pub human_kind: Option<String>,
    pub human: Option<String>,
    /// A person's note for the next stage (a rejected spec, a sent-back diff).
    pub note: Option<String>,
    pub approver: Option<Value>,
    pub summary: Option<String>,
    /// Brain checks a person said not to run for this order («Ignorar en
    /// esta orden»). Never a tester's command: the suite has no off switch.
    pub waived: Vec<String>,
    /// Brain checks a person filed as broken («El check está mal»): they
    /// don't run until the order goes on with a newer brain.
    pub broken: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GateResult {
    pub name: String,
    pub command: String,
    pub ok: bool,
    /// For a red one, what failed first, then how the output ended.
    pub tail: String,
    /// The text of the brain rules behind it, if it's a rule's check.
    #[serde(default)]
    pub rule: Option<String>,
    /// Rounds in a row it has been red, this one included.
    #[serde(default)]
    pub reds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: String,
    pub project_id: String,
    pub order_id: String,
    /// `running`, `waiting_limit`, `waiting_human`, `stopped`, `failed`,
    /// `delivered` (MR open) or `merged`.
    pub status: String,
    pub stage: String,
    pub round: i64,
    pub team: Vec<Agent>,
    pub brain_version: i64,
    pub branch: String,
    pub worktree: String,
    pub base: String,
    pub resume_at: Option<String>,
    pub mr_url: Option<String>,
    pub mr_number: Option<i64>,
    pub cost_usd: f64,
    pub error: Option<String>,
    pub state: RunState,
    pub started_at: String,
    pub finished_at: Option<String>,
}

const RUN_COLS: &str = "id, project_id, order_id, status, stage, round, team, brain_version, branch, worktree, base, resume_at, mr_url, mr_number, cost_usd, error, state, started_at, finished_at";

fn run_from_row(r: &rusqlite::Row) -> rusqlite::Result<Run> {
    Ok(Run {
        id: r.get(0)?,
        project_id: r.get(1)?,
        order_id: r.get(2)?,
        status: r.get(3)?,
        stage: r.get(4)?,
        round: r.get(5)?,
        team: serde_json::from_str(&r.get::<_, String>(6)?).unwrap_or_default(),
        brain_version: r.get(7)?,
        branch: r.get(8)?,
        worktree: r.get(9)?,
        base: r.get(10)?,
        resume_at: r.get(11)?,
        mr_url: r.get(12)?,
        mr_number: r.get(13)?,
        cost_usd: r.get(14)?,
        error: r.get(15)?,
        state: serde_json::from_str(&r.get::<_, String>(16)?).unwrap_or_default(),
        started_at: r.get(17)?,
        finished_at: r.get(18)?,
    })
}

pub fn load_run(conn: &Connection, id: &str) -> Result<Run, String> {
    conn.query_row(&format!("SELECT {RUN_COLS} FROM factory_runs WHERE id = ?1"), params![id], run_from_row).map_err(|_| "esa ejecución ya no existe".to_string())
}

/// The run of the order the project is working on (or waiting on), if any.
pub fn active_run(conn: &Connection, project_id: &str) -> Result<Option<Run>, String> {
    conn.query_row(
        &format!("SELECT {RUN_COLS} FROM factory_runs WHERE project_id = ?1 AND status IN ('running','waiting_limit','waiting_human','stopped') ORDER BY started_at DESC LIMIT 1"),
        params![project_id],
        run_from_row,
    )
    .optional()
    .map_err(|e| e.to_string())
}

pub fn runs(conn: &Connection, project_id: &str, limit: i64) -> Result<Vec<Run>, String> {
    let mut stmt = conn.prepare(&format!("SELECT {RUN_COLS} FROM factory_runs WHERE project_id = ?1 ORDER BY started_at DESC LIMIT ?2")).map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![project_id, limit], run_from_row).map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

pub fn save_run(conn: &Connection, run: &Run) -> Result<(), String> {
    conn.execute(
        "UPDATE factory_runs SET status = ?2, stage = ?3, round = ?4, resume_at = ?5, mr_url = ?6, mr_number = ?7, cost_usd = ?8, error = ?9, state = ?10, finished_at = ?11, team = ?12 WHERE id = ?1",
        params![
            run.id,
            run.status,
            run.stage,
            run.round,
            run.resume_at,
            run.mr_url,
            run.mr_number,
            run.cost_usd,
            run.error,
            serde_json::to_string(&run.state).unwrap_or_else(|_| "{}".into()),
            run.finished_at,
            serde_json::to_string(&run.team).unwrap_or_else(|_| "[]".into())
        ],
    )
    .map_err(|e| e.to_string())?;
    // The order mirrors its run, so the queue and the provider read the same.
    let order_status = match run.status.as_str() {
        "running" => "running",
        other => other,
    };
    orders::set_status(conn, &run.order_id, order_status)
}

/// What the line needs from Hangar, passed in so its tests can stand in for
/// every piece of it.
pub struct Ctx<'a> {
    pub db: &'a DbConnection,
    pub runner: &'a dyn Runner,
    /// Identity and push credential for this project's git.
    pub git_env: Vec<(String, String)>,
    /// App data, for the Chrome MCP config.
    pub root: Option<PathBuf>,
    /// An OS + in-app notification: title, body.
    pub notify: &'a (dyn Fn(&str, &str) + Sync),
    /// Something the live view shows changed.
    pub changed: &'a (dyn Fn() + Sync),
}

fn lock(db: &DbConnection) -> Result<std::sync::MutexGuard<'_, Connection>, String> {
    db.0.lock().map_err(|e| e.to_string())
}

fn git(cwd: &Path, args: &[&str], env: &[(String, String)]) -> Result<String, String> {
    crate::commands::pull_requests::git_capture_with_env(cwd, args, env)
}

/// `Cancelar una cita (DELETE /citas/{id})` → `cancelar-una-cita-delete-citas-id`, ≤ 40.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.to_lowercase().chars() {
        let c = match c {
            'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ë' => 'e',
            'í' | 'ì' | 'ï' => 'i',
            'ó' | 'ò' | 'ö' => 'o',
            'ú' | 'ù' | 'ü' => 'u',
            'ñ' => 'n',
            c => c,
        };
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let mut s: String = out.trim_matches('-').chars().take(40).collect();
    while s.ends_with('-') {
        s.pop();
    }
    s
}

pub fn branch_name(order: &Order) -> String {
    // Lowercase, like the slug: brains ask for `feat/[a-z0-9-]+`.
    let key: String = order.key.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').map(|c| c.to_ascii_lowercase()).collect();
    format!("feat/{key}-{}", slug(&order.title))
}

/// Whether the project can run orders at all: a git repo with commits, a
/// sealed brain and a team that can work. What fails here is the project's,
/// not any one order's.
pub fn ready(db: &DbConnection, project_id: &str) -> Result<(), String> {
    let (path, brain_version, problem) = {
        let conn = lock(db)?;
        let project = super::store::get(&conn, project_id)?;
        let check = super::team::check(&conn, &super::team::load(&conn, project_id)?)?;
        (project.project_path, project.brain_version, check.problems.into_iter().next())
    };
    // git runs without the database lock: everything else in Hangar reads it.
    let status = super::setup::repo_status(Path::new(&path));
    if !status.is_repo || !status.has_commits {
        return Err("la carpeta del proyecto no es un repo git con commits: inicialízalo en el paso Proyecto (main + dev)".into());
    }
    if brain_version.is_none() {
        return Err("sella antes el brain: la línea decide con él".into());
    }
    if let Some(problem) = problem {
        return Err(format!("el equipo no puede trabajar: {problem}"));
    }
    Ok(())
}

/// Starts the order: freezes the brain and the team, cuts its branch from the
/// base and opens its worktree. Refuses when the line can't run.
pub fn start(ctx: &Ctx, project_id: &str, order_id: &str) -> Result<Run, String> {
    let (project, order, team) = {
        let conn = lock(ctx.db)?;
        let project = super::store::get(&conn, project_id)?;
        let order = orders::get(&conn, order_id)?;
        let team = super::team::load(&conn, project_id)?;
        let check = super::team::check(&conn, &team)?;
        if let Some(problem) = check.problems.first() {
            return Err(format!("el equipo no puede trabajar: {problem}"));
        }
        (project, order, team)
    };
    let brain_version = order.brain_version.or(project.brain_version).ok_or("sella antes el brain: la línea decide con él")?;
    let repo = Path::new(&project.project_path);
    let status = super::setup::repo_status(repo);
    if !status.is_repo || !status.has_commits {
        return Err("la carpeta del proyecto no es un repo git con commits".into());
    }
    let base_branch = status.base_branch.clone().unwrap_or_else(|| "main".into());
    // With a remote, from what's on it; the local base may be behind.
    let base = if status.remote_url.is_some() && git(repo, &["fetch", "origin", &base_branch], &ctx.git_env).is_ok() {
        format!("origin/{base_branch}")
    } else {
        base_branch.clone()
    };
    let branch = branch_name(&order);
    let worktree = repo.join(".hangar").join("worktrees").join(format!("factory-{}", slug(&order.key)));
    crate::task_sources::exclude_hangar_dir_locally(repo);
    if !worktree.join(".git").exists() {
        let dir = worktree.to_string_lossy().to_string();
        std::fs::create_dir_all(worktree.parent().expect("has a parent")).map_err(|e| e.to_string())?;
        let exists = git(repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")], &[]).is_ok();
        let added = if exists { git(repo, &["worktree", "add", &dir, &branch], &[]) } else { git(repo, &["worktree", "add", &dir, "-b", &branch, &base], &[]) };
        if let Err(e) = added {
            // The order's own failure: kept as a run, so Entregas says why.
            let why = format!("no se pudo abrir el worktree de {}: {e}", order.key);
            let conn = lock(ctx.db)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO factory_runs (id, project_id, order_id, status, stage, round, team, brain_version, branch, worktree, base, error, state, started_at, finished_at)
                 VALUES (?1, ?2, ?3, 'failed', 'spec', 0, '[]', ?4, ?5, ?6, ?7, ?8, '{}', ?9, ?9)",
                params![uuid::Uuid::new_v4().to_string(), project_id, order_id, brain_version, branch, worktree.to_string_lossy(), base_branch, why, now],
            )
            .map_err(|e| e.to_string())?;
            orders::set_status(&conn, order_id, "failed")?;
            return Err(why);
        }
    }

    // A fresh worktree has no node_modules or virtualenv: install them now, so
    // the tests and checks can run. Not fatal — the builder can try again.
    let installed = super::env::install_deps(project_id, &worktree);

    let run = Run {
        id: uuid::Uuid::new_v4().to_string(),
        project_id: project_id.into(),
        order_id: order_id.into(),
        status: "running".into(),
        stage: "spec".into(),
        round: 0,
        team,
        brain_version,
        branch,
        worktree: worktree.to_string_lossy().to_string(),
        base: base_branch,
        resume_at: None,
        mr_url: None,
        mr_number: None,
        cost_usd: 0.0,
        error: None,
        state: RunState::default(),
        started_at: Utc::now().to_rfc3339(),
        finished_at: None,
    };
    let conn = lock(ctx.db)?;
    conn.execute(
        "INSERT INTO factory_runs (id, project_id, order_id, status, stage, round, team, brain_version, branch, worktree, base, state, started_at)
         VALUES (?1, ?2, ?3, 'running', 'spec', 0, ?4, ?5, ?6, ?7, ?8, '{}', ?9)",
        params![run.id, run.project_id, run.order_id, serde_json::to_string(&run.team).unwrap_or_default(), brain_version, run.branch, run.worktree, run.base, run.started_at],
    )
    .map_err(|e| e.to_string())?;
    orders::freeze_brain(&conn, order_id, brain_version)?;
    orders::set_status(&conn, order_id, "running")?;
    activity(&conn, &run.id, None, "ing", "cmd", &format!("git worktree add … {} desde {base}", run.branch), None)?;
    match installed {
        Some(Ok(what)) => activity(&conn, &run.id, None, "ing", "cmd", &format!("{what} en el worktree"), None)?,
        Some(Err(why)) => activity(&conn, &run.id, None, "ing", "warn", &why, None)?,
        None => {}
    }
    Ok(run)
}

fn activity(conn: &Connection, run_id: &str, step_id: Option<&str>, station: &str, kind: &str, text: &str, rule: Option<&str>) -> Result<(), String> {
    conn.execute(
        "INSERT INTO factory_activity (run_id, step_id, station, at, kind, text, rule_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![run_id, step_id, station, Utc::now().to_rfc3339(), kind, text, rule],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

fn start_step(conn: &Connection, run: &Run, agent_id: Option<&str>, label: &str, model: &str, station: &str) -> Result<String, String> {
    let id = uuid::Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO factory_steps (id, run_id, agent_id, label, model, station, status, round, started_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'running', ?7, ?8)",
        params![id, run.id, agent_id, label, model, station, run.round, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(id)
}

fn finish_step(conn: &Connection, id: &str, status: &str, note: Option<&str>, session: Option<&str>, cost: f64) -> Result<(), String> {
    conn.execute(
        "UPDATE factory_steps SET status = ?2, note = COALESCE(?3, note), session_id = COALESCE(?4, session_id), cost_usd = cost_usd + ?5, finished_at = ?6 WHERE id = ?1",
        params![id, status, note, session, cost, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

// ---------------------------------------------------------------- prompts

fn decisions_schema() -> Value {
    json!({ "type": "array", "items": { "type": "object", "required": ["rule_id", "text"], "properties": {
        "rule_id": { "type": ["string", "null"] }, "text": { "type": "string" }, "question": { "type": ["string", "null"] } } } })
}

fn schema(props: Value, required: &[&str]) -> Value {
    let mut p = props;
    p["decisions"] = decisions_schema();
    let mut req: Vec<&str> = required.to_vec();
    req.push("decisions");
    json!({ "type": "object", "required": req, "properties": p })
}

pub fn role_schema(role: &str) -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    match role {
        "spec" => schema(
            json!({ "summary": {"type":"string"}, "steps": strings, "files": strings, "tests": strings, "risk": {"type":"boolean"}, "risk_reason": {"type":"string"} }),
            &["summary", "steps", "files", "tests", "risk", "risk_reason"],
        ),
        "critic" => schema(json!({ "ok": {"type":"boolean"}, "issues": strings, "revised_summary": {"type":"string"} }), &["ok", "issues", "revised_summary"]),
        "builder" | "tester" => schema(json!({ "summary": {"type":"string"}, "tests": strings }), &["summary", "tests"]),
        "verifier" => schema(
            json!({ "confirmed": {"type":"array","items":{"type":"integer"}}, "dismissed": {"type":"array","items":{"type":"object","required":["index","reason"],"properties":{"index":{"type":"integer"},"reason":{"type":"string"}}}} }),
            &["confirmed", "dismissed"],
        ),
        "approver" => schema(json!({ "decision": {"type":"string","enum":["ship","ask"]}, "reason": {"type":"string"} }), &["decision", "reason"]),
        // The four reviewers.
        _ => schema(
            json!({ "summary": {"type":"string"}, "warnings": strings, "findings": {"type":"array","items":{"type":"object","required":["rule_id","severity","file","text"],"properties":{
                "rule_id": {"type":["string","null"]}, "severity": {"type":"string","enum":["blocker","major","minor"]}, "file": {"type":"string"}, "text": {"type":"string"} }}} }),
            &["summary", "warnings", "findings"],
        ),
    }
}

fn job(role: &str) -> &'static str {
    match role {
        "spec" => "Escribe la spec de esta orden: qué se cambia y dónde (archivos), los pasos, y qué test cubre cada criterio de aceptación. Marca risk=true si el brain (Riesgo) obliga a parar y preguntar. No escribas código.",
        "critic" => "Revisa la spec contra el brain. Si rompe una regla, ok=false, lista los problemas y da en revised_summary la spec corregida. Si no, ok=true.",
        "builder" => "Implementa la spec en este worktree, en commits pequeños (git add + git commit) con mensajes en inglés. No escribas los tests de aceptación: lo hacen los testers. Si hay fallos o hallazgos que corregir, corrígelos.",
        "tester" => "Escribe los tests que te tocan para cubrir cada criterio de aceptación (cada criterio → al menos un test), ejecútalos con tu comando y haz commit de los tests. No cambies el código de producción.",
        "reviewer_code" => "Revisa el diff de la rama contra la base: errores, legibilidad y las reglas de tus secciones. Solo hallazgos reales, cada uno con la regla que rompe.",
        "reviewer_security" => "Revisa el diff con ojos de seguridad: entradas sin validar, acceso a datos de otros, secretos, auth. Cada hallazgo con la regla que rompe.",
        "reviewer_ui" => "Revisa la interfaz que toca la orden: abre la app con la herramienta de Chrome, mira las vistas afectadas y compáralas con el producto. Cada hallazgo con su regla. Si no puedes abrir la app, dilo en warnings y revisa el código de la interfaz.",
        "reviewer_ux" => "Revisa los flujos y los textos que toca la orden: abre la app con la herramienta de Chrome y recórrelos. Cada hallazgo con su regla. Si no puedes abrir la app, dilo en warnings y revisa el código.",
        "verifier" => "Te dan hallazgos de los revisores. Para cada uno, comprueba en el código si es real y si la regla citada de verdad lo exige. confirmed = índices reales; dismissed = los demás con el porqué.",
        "approver" => "Los tests están en verde y no quedan hallazgos. Decide si la orden se entrega (ship) o si una persona tiene que mirarla antes (ask), según el brain (Riesgo, Git y entrega).",
        _ => "",
    }
}

/// Everything an agent is told: who it is, the order, only its sections of
/// the frozen brain, what came before, and how to answer.
pub fn prompt(agent: &Agent, order: &Order, brain: &Sealed, base: &str, extra: &str) -> String {
    let criteria: String = order.criteria.iter().map(|c| format!("- {c}\n")).collect();
    let rules = if order.rules.is_empty() { String::new() } else { format!("Reglas que el planificador vio aplicables: {}\n", order.rules.join(", ")) };
    format!(
        "Eres «{name}» ({duty}) en la línea de la Factory de Hangar. Trabajas en la orden {key} · {title}, en la rama actual, que sale de {base}.\n\n\
TU TAREA: {job}\n\n\
DECIDE CON EL BRAIN. El brain de abajo (v{ver}, solo tus secciones) es la fuente de la verdad. Toda decisión que tomes va en `decisions` con el ID \
de la regla que la justifica (rule_id). Si ninguna regla lo cubre: rule_id null, toma la opción más conservadora, descríbela en text y escribe la \
pregunta abierta en question — irá a la bandeja de enmiendas.\n\n\
=== ORDEN {key} ===\n{title}\n{desc}\nCriterios de aceptación:\n{criteria}{rules}\n\
=== BRAIN v{ver} ===\n{brain}\n{extra}",
        name = agent.name,
        duty = agent.duty,
        key = order.key,
        title = order.title,
        desc = order.description,
        job = job(&agent.role),
        ver = brain.version.version,
        brain = brain::excerpt(brain, &agent.sections),
    )
}

// ---------------------------------------------------------------- one agent

enum Step {
    /// The agent answered.
    Answer(Value),
    /// The run stopped to wait (limit, credits, ⏹) or failed; already saved.
    Halted,
}

/// Runs one agent of the frozen team and records everything it did.
fn run_agent(ctx: &Ctx, run: &mut Run, agent: &Agent, order: &Order, brain: &Sealed, extra: &str, write: bool, mcp: Option<PathBuf>, label: Option<&str>) -> Result<Step, String> {
    let st = station(&run.stage);
    let account = run.state.accounts.get(&agent.id).cloned().or_else(|| agent.account_id.clone()).ok_or_else(|| format!("{}: sin cuenta", agent.name))?;
    let mut model = agent.model.clone();
    if agent.role == "builder" && run.state.escalated {
        if let Some(up) = &agent.escalate_model {
            model = up.clone();
        }
    }
    let resume = run.state.sessions.get(&agent.id).cloned().filter(|_| !run.state.accounts.contains_key(&agent.id) || run.state.sessions.contains_key(&format!("{}@{account}", agent.id)));
    let step_id = {
        let conn = lock(ctx.db)?;
        start_step(&conn, run, Some(&agent.id), label.unwrap_or(&agent.name), &model, st)?
    };
    (ctx.changed)();
    let mut text = prompt(agent, order, brain, &run.base, extra);
    if resume.is_some() {
        text = format!("Sigue con la orden donde la dejaste. Esto es lo que te toca ahora:\n\n{text}");
    }
    let call = AgentCall {
        project: run.project_id.clone(),
        key: run.id.clone(),
        account_id: account.clone(),
        model,
        cwd: PathBuf::from(&run.worktree),
        prompt: text,
        schema: role_schema(&agent.role),
        resume,
        mcp_config: mcp,
        write,
        timeout: AGENT_TIMEOUT,
    };
    let result = {
        let db = ctx.db;
        let run_id = run.id.clone();
        let sid = step_id.clone();
        let changed = ctx.changed;
        ctx.runner.run(&call, &mut |a| {
            if let Ok(conn) = db.0.lock() {
                let _ = activity(&conn, &run_id, Some(&sid), st, a.kind, &a.text, None);
            }
            changed();
        })
    };
    run.cost_usd += result.cost_usd;
    if let Some(id) = &result.session_id {
        run.state.sessions.insert(agent.id.clone(), id.clone());
        run.state.sessions.insert(format!("{}@{account}", agent.id), id.clone());
    }
    // What to tell the user — sent after the lock is released, never with it
    // held: a notification writes to the database too, and the same lock
    // taken twice by one thread waits on itself forever (the whole app froze).
    let mut tell: Option<(String, String)> = None;
    let conn = lock(ctx.db)?;
    let outcome = match result.outcome {
        Outcome::Ok => {
            let json = result.json.unwrap_or(Value::Null);
            record_decisions(&conn, run, &step_id, st, order, &json)?;
            finish_step(&conn, &step_id, "done", json.get("summary").and_then(Value::as_str), result.session_id.as_deref(), result.cost_usd)?;
            Step::Answer(json)
        }
        Outcome::Limit { resume_at } => {
            finish_step(&conn, &step_id, "wait", Some("límite de uso"), result.session_id.as_deref(), result.cost_usd)?;
            run.status = "waiting_limit".into();
            run.resume_at = Some(resume_at.to_rfc3339());
            activity(&conn, &run.id, Some(&step_id), st, "warn", &format!("límite de uso de la cuenta: espera y sigue sola a las {}", resume_at.with_timezone(&chrono::Local).format("%H:%M")), None)?;
            tell = Some((format!("{} espera por límite", order.key), format!("{} llegó al límite de su cuenta; sigue sola a las {}.", agent.name, resume_at.with_timezone(&chrono::Local).format("%H:%M"))));
            Step::Halted
        }
        Outcome::NoCredits => {
            finish_step(&conn, &step_id, "failed", Some("sin créditos"), result.session_id.as_deref(), result.cost_usd)?;
            run.status = "waiting_human".into();
            run.state.human_kind = Some("credits".into());
            run.state.human = Some(format!("La cuenta de {} no tiene créditos. Recarga y pulsa Reintentar, o usa otra cuenta.", agent.name));
            tell = Some((format!("{}: sin créditos", order.key), format!("La cuenta de {} no tiene créditos.", agent.name)));
            Step::Halted
        }
        Outcome::Stopped => {
            finish_step(&conn, &step_id, "stopped", Some("parada"), result.session_id.as_deref(), result.cost_usd)?;
            run.status = "stopped".into();
            Step::Halted
        }
        Outcome::Failed(why) => {
            finish_step(&conn, &step_id, "failed", Some(&why), result.session_id.as_deref(), result.cost_usd)?;
            run.status = "failed".into();
            run.error = Some(format!("{}: {why}", agent.name));
            run.finished_at = Some(Utc::now().to_rfc3339());
            tell = Some((format!("{} falló", order.key), format!("{}: {why}", agent.name)));
            Step::Halted
        }
    };
    save_run(&conn, run)?;
    drop(conn);
    if let Some((title, body)) = tell {
        (ctx.notify)(&title, &body);
    }
    (ctx.changed)();
    Ok(outcome)
}

/// Every decision with the rule it cites; one without a rule goes to the
/// amendments inbox with what was done meanwhile.
fn record_decisions(conn: &Connection, run: &Run, step_id: &str, st: &str, order: &Order, json: &Value) -> Result<(), String> {
    let known: std::collections::HashSet<String> = {
        let mut stmt = conn.prepare("SELECT rule_id FROM factory_brain_rules WHERE project_id = ?1 AND version = ?2").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![run.project_id, run.brain_version], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    for d in json.get("decisions").and_then(Value::as_array).cloned().unwrap_or_default() {
        let text = d.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if text.is_empty() {
            continue;
        }
        // A rule that isn't in the frozen version is no rule at all.
        let rule = d.get("rule_id").and_then(Value::as_str).map(str::trim).filter(|r| known.contains(*r)).map(str::to_string);
        conn.execute(
            "INSERT INTO factory_decisions (id, run_id, step_id, station, at, rule_id, text) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![uuid::Uuid::new_v4().to_string(), run.id, step_id, st, Utc::now().to_rfc3339(), rule, text],
        )
        .map_err(|e| e.to_string())?;
        match &rule {
            Some(r) => activity(conn, &run.id, Some(step_id), st, "brain", &text, Some(r))?,
            None => {
                let question = d.get("question").and_then(Value::as_str).filter(|q| !q.trim().is_empty()).unwrap_or(&text);
                activity(conn, &run.id, Some(step_id), st, "warn", &format!("sin regla: {text} → a la bandeja"), None)?;
                brain::add_amendment(conn, &run.project_id, question, Some(&order.key), Some(&text))?;
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- the gate

/// How the gate judges a command: green, or — for order 0, which only sets
/// the project up — that it runs at all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GateMode {
    Green,
    Runs,
}

/// The gate's commands for an order: every active tester's, and the
/// `check:` of the rules that apply to it — the planner's and the ones its
/// agents cited. `all` takes every check of the brain (order 0, and the
/// full verification). A command shared by several rules runs once.
pub fn gate_commands(team: &[Agent], brain: &Sealed, rules: &std::collections::HashSet<String>, all: bool) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut add = |name: &str, command: &str| match out.iter_mut().find(|(_, c)| c == command) {
        Some((names, _)) => {
            names.push_str(", ");
            names.push_str(name);
        }
        None => out.push((name.to_string(), command.to_string())),
    };
    for a in team.iter().filter(|a| a.role == "tester" && a.active) {
        if let Some(c) = &a.command {
            add(&a.name, c);
        }
    }
    for r in brain.rules.iter().filter(|r| all || rules.contains(&r.id)) {
        if let Some(c) = &r.check {
            add(&r.id, c);
        }
    }
    out
}

/// Output that says the command didn't run at all, rather than ran and failed.
pub fn not_runnable(code: Option<i32>, output: &str) -> bool {
    let lower = output.to_lowercase();
    code == Some(127)
        || lower.contains("command not found")
        || lower.contains("err_pnpm_no_script")
        || lower.contains("missing script")
        || lower.contains("cannot find module")
        || regex::Regex::new(r#"command "[^"]*" not found"#).map(|re| re.is_match(&lower)).unwrap_or(false)
}

/// The test gate: the commands, run in the worktree with no AI and the
/// user's PATH. It has no off switch.
pub fn run_gate(project: &str, worktree: &Path, commands: &[(String, String)], mode: GateMode) -> Vec<GateResult> {
    commands
        .iter()
        // ⏹ between commands: what ran is reported, the rest doesn't start.
        .take_while(|_| !super::procs::stop_requested(project))
        .map(|(name, command)| {
            let (ok, tail) = match super::env::shell(project, "gate", command, worktree, GATE_TIMEOUT) {
                Ok(out) => {
                    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                    let tail = if out.status.success() { last_lines(&text, 30) } else { failure_excerpt(&text) };
                    let missing = not_runnable(out.status.code(), &text);
                    let tail = if missing { format!("no se puede ejecutar: {command}\n{tail}") } else { tail };
                    let ok = match mode {
                        GateMode::Green => out.status.success(),
                        GateMode::Runs => !missing,
                    };
                    (ok, tail)
                }
                Err(e) => (false, e),
            };
            GateResult { name: name.clone(), command: command.clone(), ok, tail, rule: None, reds: 0 }
        })
        .collect()
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// What a red command's output says failed — the test runners' and
/// compilers' failure lines first, then how it ended. A plain tail loses
/// Playwright's list of failed tests under its summary and the web server's
/// noise.
pub fn failure_excerpt(text: &str) -> String {
    let re = regex::Regex::new(r"(?i)(✘|✗|×|\bFAIL\b|\d+ failed|\d+ flaky|^\s*\d+\) |error TS\d+|\berror:|Error:|fatal:|AssertionError|expect\(|Timeout.*exceeded|ELIFECYCLE)").expect("valid regex");
    let mut first: Vec<&str> = Vec::new();
    for l in text.lines() {
        if re.is_match(l) && !first.contains(&l) {
            first.push(l);
            if first.len() == 40 {
                break;
            }
        }
    }
    let end = last_lines(text, 15);
    if first.is_empty() {
        return end;
    }
    format!("{}\n…\n{end}", first.join("\n"))
}

/// The gate's commands without the checks a person waived for this order. A
/// command shared with a rule still waived-in, or with a tester, still runs.
pub fn without_waived(commands: Vec<(String, String)>, waived: &[String]) -> Vec<(String, String)> {
    commands
        .into_iter()
        .filter_map(|(names, command)| {
            let kept: Vec<&str> = names.split(", ").filter(|n| !waived.iter().any(|w| w == n)).collect();
            (!kept.is_empty()).then(|| (kept.join(", "), command))
        })
        .collect()
}

// ---------------------------------------------------------------- UI/UX

/// The Chrome tool the UI and UX reviewers get, as an MCP config.
fn chrome_mcp(root: &Path) -> Result<PathBuf, String> {
    let path = root.join("factory").join("chrome-mcp.json");
    std::fs::create_dir_all(path.parent().expect("has a parent")).map_err(|e| e.to_string())?;
    let npx = crate::providers::resolve_binary("npx");
    let config = json!({ "mcpServers": { "chrome-devtools": { "command": npx, "args": ["-y", "chrome-devtools-mcp@latest", "--isolated"] } } });
    std::fs::write(&path, config.to_string()).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Starts the project's app in the worktree and waits for its URL to answer.
/// The child is killed when the guard drops.
pub struct AppGuard(Option<super::procs::Tracked>);

impl Drop for AppGuard {
    fn drop(&mut self) {
        // The whole group: a dev server's children go with it.
        if let Some(t) = &mut self.0 {
            super::procs::kill_group(t.id());
            let _ = t.child.wait();
        }
    }
}

pub fn open_app(project: &str, worktree: &Path, start_cmd: Option<&str>, url: Option<&str>) -> Result<AppGuard, String> {
    let url = url.filter(|u| !u.trim().is_empty()).ok_or("el proyecto no tiene URL de la app (paso Proyecto)")?;
    if crate::providers::resolve_binary("npx") == "npx" && which("npx").is_none() {
        return Err("no hay npx para la herramienta de Chrome".into());
    }
    let child = match start_cmd.filter(|c| !c.trim().is_empty()) {
        Some(cmd) => {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", cmd]).current_dir(worktree).env("PATH", super::env::user_path()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
            Some(super::procs::spawn(project, "app", &mut c).map_err(|e| format!("no arrancó «{cmd}»: {e}"))?)
        }
        None => None,
    };
    let guard = AppGuard(child);
    let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(3)).build().map_err(|e| e.to_string())?;
    for _ in 0..40 {
        if client.get(url).send().is_ok() {
            return Ok(guard);
        }
        if super::procs::stop_requested(project) {
            return Err("parado".into());
        }
        std::thread::sleep(Duration::from_millis(1500));
    }
    Err(format!("{url} no respondió en 60 s"))
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|p| p.join(bin)).find(|p| p.is_file()))
}

// ---------------------------------------------------------------- delivery

/// Merges `branch` into the local `base`, never pushing. Where `base` is
/// checked out it merges there (and refuses to touch uncommitted work);
/// elsewhere it builds the merge commit without a checkout.
pub fn merge_local(repo: &Path, base: &str, branch: &str, message: &str, env: &[(String, String)]) -> Result<(), String> {
    let list = git(repo, &["worktree", "list", "--porcelain"], &[])?;
    let mut checked_out: Option<PathBuf> = None;
    let mut current: Option<PathBuf> = None;
    for line in list.lines() {
        if let Some(p) = line.strip_prefix("worktree ") {
            current = Some(PathBuf::from(p));
        } else if line.trim() == format!("branch refs/heads/{base}") {
            checked_out = current.clone();
        }
    }
    match checked_out {
        Some(dir) => {
            if !git(&dir, &["status", "--porcelain", "--untracked-files=no"], &[])?.trim().is_empty() {
                return Err(format!("tu copia de {base} ({}) tiene cambios sin commit; la fusión espera a que los guardes", dir.display()));
            }
            if let Err(e) = git(&dir, &["merge", "--no-ff", "--no-edit", "-m", message, branch], env) {
                let _ = git(&dir, &["merge", "--abort"], &[]);
                return Err(format!("conflicto al fusionar en {base}: {e}"));
            }
            Ok(())
        }
        None => {
            let old = git(repo, &["rev-parse", &format!("refs/heads/{base}")], &[])?.trim().to_string();
            let tree = git(repo, &["merge-tree", "--write-tree", base, branch], &[]).map_err(|e| format!("conflicto al fusionar en {base}: {e}"))?;
            let tree = tree.lines().next().unwrap_or("").trim().to_string();
            let commit = git(repo, &["commit-tree", &tree, "-p", base, "-p", branch, "-m", message], env)?.trim().to_string();
            git(repo, &["update-ref", &format!("refs/heads/{base}"), &commit, &old], &[])?;
            Ok(())
        }
    }
}

fn mr_body(order: &Order, run: &Run, decisions: &[(Option<String>, String)]) -> String {
    let mut b = format!("{}\n\n", run.state.summary.clone().unwrap_or_else(|| order.title.clone()));
    if let Some(url) = &order.ext_url {
        b.push_str(&format!("Tarea: {url}\n\n"));
    }
    if !order.criteria.is_empty() {
        b.push_str("**Criterios de aceptación** (cubiertos por tests, suite en verde)\n");
        for c in &order.criteria {
            b.push_str(&format!("- [x] {c}\n"));
        }
        b.push('\n');
    }
    if !decisions.is_empty() {
        b.push_str(&format!("**Decisiones** (brain v{})\n", run.brain_version));
        for (rule, text) in decisions {
            b.push_str(&format!("- {} {text}\n", rule.as_deref().map(|r| format!("`{r}`")).unwrap_or_else(|| "_sin regla_".into())));
        }
        b.push('\n');
    }
    if let Some(reason) = run.state.approver.as_ref().and_then(|a| a.get("reason")).and_then(Value::as_str) {
        b.push_str(&format!("**Aprobador:** {reason}\n\n"));
    }
    if !run.state.warnings.is_empty() {
        b.push_str("**Avisos**\n");
        for w in &run.state.warnings {
            b.push_str(&format!("- {w}\n"));
        }
        b.push('\n');
    }
    b.push_str(&format!("_{} rondas · Hangar Factory_", run.round));
    b
}

// ---------------------------------------------------------------- driving

/// Runs the order's stages until it's delivered, waits or fails. Safe to call
/// again on a waiting or stopped run: it picks up at the stage it was at.
pub fn advance(ctx: &Ctx, run_id: &str) -> Result<Run, String> {
    let (mut run, order, brain, project) = {
        let conn = lock(ctx.db)?;
        let run = load_run(&conn, run_id)?;
        let order = orders::get(&conn, &run.order_id)?;
        let brain = brain::load(&conn, &run.project_id, Some(run.brain_version))?.ok_or("la versión del brain de esta orden ya no está")?;
        let project = super::store::get(&conn, &run.project_id)?;
        (run, order, brain, project)
    };
    if run.status != "running" {
        run.status = "running".into();
        run.resume_at = None;
        run.state.human = None;
        run.state.human_kind = None;
        let conn = lock(ctx.db)?;
        save_run(&conn, &run)?;
    }
    let team = run.team.clone();
    let agent = |role: &str| team.iter().find(|a| a.role == role && a.active).cloned();
    // Order 0 sets the project up: no testers, and a gate that only asks
    // every command to run.
    let bootstrap = order.origin == "bootstrap";
    // A notification a stage decides on while it holds the database lock,
    // sent after it lets go.
    let mut tell: Option<(String, String)> = None;

    loop {
        if run.status != "running" {
            return Ok(run);
        }
        // ⏹ between stages: stop here, at this stage, to go on later.
        if super::procs::stop_requested(&run.project_id) {
            run.status = "stopped".into();
            let conn = lock(ctx.db)?;
            save_run(&conn, &run)?;
            drop(conn);
            (ctx.changed)();
            return Ok(run);
        }
        let stage = run.stage.clone();
        match stage.as_str() {
            "spec" => {
                let a = agent("spec").ok_or("el equipo no tiene spec writer")?;
                let note = run.state.note.take().map(|n| format!("\nUna persona rechazó la spec anterior: {n}\n")).unwrap_or_default();
                match run_agent(ctx, &mut run, &a, &order, &brain, &note, false, None, None)? {
                    Step::Answer(spec) => {
                        run.state.summary = spec.get("summary").and_then(Value::as_str).map(str::to_string);
                        run.state.spec = Some(spec);
                        run.stage = "critic".into();
                    }
                    Step::Halted => return Ok(run),
                }
            }
            "critic" => {
                if let Some(a) = agent("critic") {
                    let extra = format!("\n=== SPEC ===\n{}\n", run.state.spec.clone().unwrap_or_default());
                    match run_agent(ctx, &mut run, &a, &order, &brain, &extra, false, None, None)? {
                        Step::Answer(r) => {
                            if r.get("ok").and_then(Value::as_bool) == Some(false) {
                                if let (Some(spec), Some(fixed)) = (run.state.spec.as_mut(), r.get("revised_summary").and_then(Value::as_str)) {
                                    spec["summary"] = json!(fixed);
                                    spec["critic_issues"] = r.get("issues").cloned().unwrap_or(json!([]));
                                }
                            }
                        }
                        Step::Halted => return Ok(run),
                    }
                }
                run.stage = "spec_review".into();
            }
            "spec_review" => {
                let risky = run.state.spec.as_ref().and_then(|s| s.get("risk")).and_then(Value::as_bool).unwrap_or(false);
                let stop = match project.mode.as_str() {
                    "supervised" => true,
                    "delegated" => risky,
                    _ => false,
                };
                let conn = lock(ctx.db)?;
                if stop {
                    let reason = if risky {
                        run.state.spec.as_ref().and_then(|s| s.get("risk_reason")).and_then(Value::as_str).unwrap_or("el brain marca riesgo").to_string()
                    } else {
                        "modo supervisado: apruebas cada spec".into()
                    };
                    let step = start_step(&conn, &run, None, "✋ Spec", "", "ing")?;
                    finish_step(&conn, &step, "human", Some(&reason), None, 0.0)?;
                    run.status = "waiting_human".into();
                    run.state.human_kind = Some("spec_review".into());
                    run.state.human = Some(format!("Revisa la spec de {}: {reason}", order.key));
                    run.stage = "build".into();
                    save_run(&conn, &run)?;
                    drop(conn);
                    (ctx.notify)(&format!("✋ {} te espera", order.key), "Revisa la spec antes de fabricar.");
                    (ctx.changed)();
                    return Ok(run);
                }
                let step = start_step(&conn, &run, None, "✋ Spec", "", "ing")?;
                finish_step(&conn, &step, "skipped", Some(&format!("saltada · modo {}", if project.mode == "delegated" { "delegado, sin riesgo" } else { "autónomo" })), None, 0.0)?;
                run.stage = "build".into();
                save_run(&conn, &run)?;
            }
            "build" => {
                let a = agent("builder").ok_or("el equipo no tiene builder")?;
                let mut extra = format!("\n=== SPEC ===\n{}\n", run.state.spec.clone().unwrap_or_default());
                if let Some(fb) = &run.state.feedback {
                    extra.push_str(&format!("\n=== QUÉ CORREGIR (ronda {}) ===\n{fb}\n", run.round));
                }
                if let Some(n) = run.state.note.take() {
                    extra.push_str(&format!("\nNota de una persona: {n}\n"));
                }
                if bootstrap {
                    let all = gate_commands(&team, &brain, &Default::default(), true);
                    extra.push_str("\n=== ORDEN 0 · PREPARAR EL PROYECTO ===\nEl repo está vacío. Crea el stack que dice el brain (Tecnologías), instala las dependencias y añade los scripts y la configuración necesarios para que TODOS estos comandos se puedan ejecutar (que existan; pueden fallar mientras no haya producto). No implementes funcionalidades del producto.\n");
                    for (names, c) in &all {
                        extra.push_str(&format!("- `{c}`  ({names})\n"));
                    }
                }
                match run_agent(ctx, &mut run, &a, &order, &brain, &extra, true, None, None)? {
                    Step::Answer(r) => {
                        if let Some(s) = r.get("summary").and_then(Value::as_str) {
                            run.state.summary = Some(s.to_string());
                        }
                        run.state.feedback = None;
                        run.state.gate_ok = false;
                        run.stage = "test".into();
                    }
                    Step::Halted => return Ok(run),
                }
            }
            "test" if bootstrap => run.stage = "gate".into(),
            "test" => {
                let testers: Vec<Agent> = team.iter().filter(|a| a.role == "tester" && a.active).cloned().collect();
                let done: std::collections::HashSet<String> = {
                    let conn = lock(ctx.db)?;
                    let mut stmt = conn
                        .prepare("SELECT agent_id FROM factory_steps WHERE run_id = ?1 AND round = ?2 AND station = 'test' AND status = 'done' AND agent_id IS NOT NULL")
                        .map_err(|e| e.to_string())?;
                    let rows = stmt.query_map(params![run.id, run.round], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
                    rows.filter_map(Result::ok).collect()
                };
                let extra = format!("\n=== SPEC ===\n{}\n", run.state.spec.clone().unwrap_or_default());
                for t in testers.iter().filter(|t| !done.contains(&t.id)) {
                    let mine = format!("{extra}\nTu comando: {}\n", t.command.clone().unwrap_or_default());
                    if let Step::Halted = run_agent(ctx, &mut run, t, &order, &brain, &mine, true, None, None)? {
                        return Ok(run);
                    }
                }
                run.stage = "gate".into();
            }
            "gate" => {
                let step = {
                    let conn = lock(ctx.db)?;
                    start_step(&conn, &run, None, "¿Suite en verde?", "sin IA", "test")?
                };
                (ctx.changed)();
                let cited: std::collections::HashSet<String> = {
                    let conn = lock(ctx.db)?;
                    let mut stmt = conn.prepare("SELECT DISTINCT rule_id FROM factory_decisions WHERE run_id = ?1 AND rule_id IS NOT NULL").map_err(|e| e.to_string())?;
                    let rows = stmt.query_map(params![run.id], |r| r.get::<_, String>(0)).map_err(|e| e.to_string())?;
                    rows.filter_map(Result::ok).chain(order.rules.iter().cloned()).collect()
                };
                let skipped: Vec<String> = run.state.waived.iter().chain(&run.state.broken).cloned().collect();
                let commands = without_waived(gate_commands(&team, &brain, &cited, bootstrap), &skipped);
                let mut results = run_gate(&run.project_id, Path::new(&run.worktree), &commands, if bootstrap { GateMode::Runs } else { GateMode::Green });
                // ⏹ in the middle of the gate: no verdict on half a suite —
                // it runs again from the start on ▶.
                if super::procs::stop_requested(&run.project_id) {
                    let conn = lock(ctx.db)?;
                    finish_step(&conn, &step, "stopped", Some("parada"), None, 0.0)?;
                    run.status = "stopped".into();
                    save_run(&conn, &run)?;
                    drop(conn);
                    (ctx.changed)();
                    return Ok(run);
                }
                for r in results.iter_mut() {
                    let texts: Vec<String> = r.name.split(", ").filter_map(|n| brain.rules.iter().find(|x| x.id == n)).map(|x| format!("{} · {}", x.id, x.text)).collect();
                    r.rule = (!texts.is_empty()).then(|| texts.join("\n"));
                    let before = run.state.gate.iter().find(|g| g.command == r.command).map(|g| g.reds).unwrap_or(0);
                    r.reds = if r.ok { 0 } else { before + 1 };
                }
                let green = !results.is_empty() && results.iter().all(|r| r.ok);
                let conn = lock(ctx.db)?;
                for r in &results {
                    let rule = brain.rules.iter().any(|x| x.id == r.name).then_some(r.name.as_str());
                    activity(&conn, &run.id, Some(&step), "test", if r.ok { "gate" } else { "warn" }, &format!("{} {} → {}", if r.ok { "✓" } else { "✗" }, r.command, if r.ok { "verde" } else { "rojo" }), rule)?;
                }
                finish_step(&conn, &step, if green { "done" } else { "failed" }, Some(&format!("{} de {} en verde", results.iter().filter(|r| r.ok).count(), results.len())), None, 0.0)?;
                run.state.gate = results.clone();
                run.state.gate_ok = green;
                if green {
                    run.stage = "review".into();
                } else {
                    let failures: Vec<String> = results.iter().filter(|r| !r.ok).map(|r| format!("✗ {} ({})\n{}", r.name, r.command, r.tail)).collect();
                    let why = if results.is_empty() { "no hay ningún tester activo con comando: sin tests no se aprueba".to_string() } else { failures.join("\n\n") };
                    tell = back_to_build(&conn, &mut run, &order, why)?;
                }
                save_run(&conn, &run)?;
                drop(conn);
                if let Some((title, body)) = tell.take() {
                    (ctx.notify)(&title, &body);
                }
            }
            "review" => {
                let mut reviewers: Vec<Agent> = team.iter().filter(|a| a.active && (a.role == "reviewer_code" || a.role == "reviewer_security")).cloned().collect();
                if order.touches_ui {
                    reviewers.extend(team.iter().filter(|a| a.active && (a.role == "reviewer_ui" || a.role == "reviewer_ux")).cloned());
                }
                let diff = git(Path::new(&run.worktree), &["diff", "--stat", &format!("{}...HEAD", run.base)], &[]).unwrap_or_default();
                let mut findings: Vec<Value> = Vec::new();
                let mut app: Option<AppGuard> = None;
                let mut app_tried = false;
                for r in &reviewers {
                    let mut mcp = None;
                    if r.role == "reviewer_ui" || r.role == "reviewer_ux" {
                        if !app_tried {
                            app_tried = true;
                            match open_app(&run.project_id, Path::new(&run.worktree), project.app_start_cmd.as_deref(), project.app_url.as_deref()) {
                                Ok(g) => app = Some(g),
                                Err(e) => {
                                    let w = format!("UI/UX: no se pudo abrir la app: {e}");
                                    let conn = lock(ctx.db)?;
                                    activity(&conn, &run.id, None, "rev", "warn", &format!("{w} · revisan el código y la orden sigue"), None)?;
                                    run.state.warnings.push(w);
                                }
                            }
                        }
                        if app.is_some() {
                            mcp = ctx.root.as_deref().and_then(|root| chrome_mcp(root).ok());
                        }
                    }
                    let mut extra = format!("\n=== CAMBIOS ({}...HEAD) ===\n{diff}\nUsa `git diff {}...HEAD` para verlos enteros.\n", run.base, run.base);
                    if mcp.is_some() {
                        extra.push_str(&format!("\nLa app está abierta en {}: ábrela con la herramienta de Chrome.\n", project.app_url.clone().unwrap_or_default()));
                    }
                    match run_agent(ctx, &mut run, r, &order, &brain, &extra, false, mcp, None)? {
                        Step::Answer(ans) => {
                            for f in ans.get("findings").and_then(Value::as_array).cloned().unwrap_or_default() {
                                let mut f = f;
                                f["by"] = json!(r.name);
                                findings.push(f);
                            }
                            for w in ans.get("warnings").and_then(Value::as_array).cloned().unwrap_or_default() {
                                if let Some(w) = w.as_str() {
                                    run.state.warnings.push(format!("{}: {w}", r.name));
                                }
                            }
                        }
                        Step::Halted => return Ok(run),
                    }
                }
                drop(app);
                run.state.findings = findings;
                run.stage = "verify".into();
                let conn = lock(ctx.db)?;
                save_run(&conn, &run)?;
            }
            "verify" => {
                let findings = run.state.findings.clone();
                let confirmed: Vec<Value> = if findings.is_empty() {
                    Vec::new()
                } else if let Some(v) = agent("verifier") {
                    let list: String = findings.iter().enumerate().map(|(i, f)| format!("{i}. {f}\n")).collect();
                    match run_agent(ctx, &mut run, &v, &order, &brain, &format!("\n=== HALLAZGOS ===\n{list}"), false, None, None)? {
                        Step::Answer(ans) => ans.get("confirmed").and_then(Value::as_array).cloned().unwrap_or_default().iter().filter_map(Value::as_u64).filter_map(|i| findings.get(i as usize).cloned()).collect(),
                        Step::Halted => return Ok(run),
                    }
                } else {
                    // Without a verifier every finding counts as real.
                    findings.clone()
                };
                let conn = lock(ctx.db)?;
                if confirmed.is_empty() {
                    run.stage = "approve".into();
                } else {
                    let list: Vec<String> = confirmed.iter().map(|f| format!("- [{}] {} {}: {}", f.get("severity").and_then(Value::as_str).unwrap_or(""), f.get("rule_id").and_then(Value::as_str).unwrap_or("sin regla"), f.get("file").and_then(Value::as_str).unwrap_or(""), f.get("text").and_then(Value::as_str).unwrap_or(""))).collect();
                    for f in &confirmed {
                        activity(&conn, &run.id, None, "dec", "warn", &format!("hallazgo confirmado: {}", f.get("text").and_then(Value::as_str).unwrap_or("")), f.get("rule_id").and_then(Value::as_str))?;
                    }
                    tell = back_to_build(&conn, &mut run, &order, format!("Hallazgos confirmados de la revisión:\n{}", list.join("\n")))?;
                }
                save_run(&conn, &run)?;
                drop(conn);
                if let Some((title, body)) = tell.take() {
                    (ctx.notify)(&title, &body);
                }
            }
            "approve" => {
                // The approver only ever sees a green suite: if the gate
                // didn't pass for this exact code, back to the gate.
                if !run.state.gate_ok {
                    run.stage = "gate".into();
                    continue;
                }
                // Without an approver the spec's risk flag stands in for its
                // «ask»: delegated mode still stops on a risky order.
                let (ask, reason) = match agent("approver") {
                    Some(a) => {
                        let extra = format!("\n=== RESUMEN ===\n{}\nSuite: {} comprobaciones en verde. Avisos: {}\n", run.state.summary.clone().unwrap_or_default(), run.state.gate.len(), run.state.warnings.join(" · "));
                        let ans = match run_agent(ctx, &mut run, &a, &order, &brain, &extra, false, None, None)? {
                            Step::Answer(ans) => ans,
                            Step::Halted => return Ok(run),
                        };
                        let ask = ans.get("decision").and_then(Value::as_str) == Some("ask");
                        let reason = ans.get("reason").and_then(Value::as_str).unwrap_or("").to_string();
                        run.state.approver = Some(ans);
                        (ask, format!("El aprobador pregunta: {reason}"))
                    }
                    None => {
                        let spec = run.state.spec.as_ref();
                        let risky = spec.and_then(|s| s.get("risk")).and_then(Value::as_bool).unwrap_or(false);
                        let why = spec.and_then(|s| s.get("risk_reason")).and_then(Value::as_str).filter(|r| !r.trim().is_empty()).unwrap_or("el brain marca riesgo");
                        (risky, format!("Sin aprobador, y la spec marca riesgo: {why}"))
                    }
                };
                let conn = lock(ctx.db)?;
                let human = match project.mode.as_str() {
                    "supervised" => Some(("diff", format!("Modo supervisado: revisa el diff de {} antes de entregar.", order.key))),
                    "delegated" if ask => Some(("approver", reason.clone())),
                    _ => None,
                };
                run.stage = "deliver".into();
                if let Some((kind, why)) = human {
                    let step = start_step(&conn, &run, None, "✋ Entrega", "", "dec")?;
                    finish_step(&conn, &step, "human", Some(&why), None, 0.0)?;
                    run.status = "waiting_human".into();
                    run.state.human_kind = Some(kind.into());
                    run.state.human = Some(why);
                    save_run(&conn, &run)?;
                    drop(conn);
                    (ctx.notify)(&format!("✋ {} te espera", order.key), "Revisa la orden antes de entregarla.");
                    (ctx.changed)();
                    return Ok(run);
                }
                if ask {
                    // Autonomous: it never stops; the risk goes in the MR.
                    run.state.warnings.push(format!("Se habría parado a preguntar. {reason}"));
                }
                save_run(&conn, &run)?;
            }
            "deliver" => {
                if !run.state.gate_ok {
                    run.stage = "gate".into();
                    continue;
                }
                deliver(ctx, &mut run, &order, &project)?;
                return Ok(run);
            }
            other => return Err(format!("etapa desconocida `{other}`")),
        }
    }
}

/// Red, or findings confirmed: back to Fabricación with what to fix. After
/// K rounds the builder escalates; after MAX_ROUNDS the order fails.
/// Returns the notification to send, if any — the caller holds the database
/// lock and sends it once it has let go.
fn back_to_build(conn: &Connection, run: &mut Run, order: &Order, why: String) -> Result<Option<(String, String)>, String> {
    run.round += 1;
    activity(conn, &run.id, None, "fab", "warn", &format!("↺ vuelve a Fabricación · ronda {}", run.round), None)?;
    if run.round > MAX_ROUNDS {
        run.status = "failed".into();
        run.error = Some(format!("sigue sin pasar tras {MAX_ROUNDS} rondas"));
        run.finished_at = Some(Utc::now().to_rfc3339());
        return Ok(Some((format!("{} falló", order.key), format!("Sigue sin pasar tras {MAX_ROUNDS} rondas."))));
    }
    if run.round > K_ROUNDS && !run.state.escalated {
        run.state.escalated = true;
        if let Some(up) = run.team.iter().find(|a| a.role == "builder").and_then(|b| b.escalate_model.clone()) {
            activity(conn, &run.id, None, "fab", "warn", &format!("el builder escala a {up}"), None)?;
        }
    }
    run.state.feedback = Some(why);
    run.state.gate_ok = false;
    run.stage = "build".into();
    Ok(None)
}

fn deliver(ctx: &Ctx, run: &mut Run, order: &Order, project: &super::store::FactoryProject) -> Result<(), String> {
    let step = {
        let conn = lock(ctx.db)?;
        start_step(&conn, run, None, "Entrega", if project.delivery == "mr" { "MR" } else { "merge local" }, "ent")?
    };
    let decisions: Vec<(Option<String>, String)> = {
        let conn = lock(ctx.db)?;
        let mut stmt = conn.prepare("SELECT rule_id, text FROM factory_decisions WHERE run_id = ?1 ORDER BY at").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![run.id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    let wt = PathBuf::from(&run.worktree);
    let title = format!("{} · {}", order.key, order.title);
    let body = mr_body(order, run, &decisions);
    let (branch, base) = (run.branch.clone(), run.base.clone());
    let result: Result<(String, Option<(String, i64)>), String> = if project.delivery == "mr" {
        (|| {
            // Only the order's own branch is ever pushed.
            git(&wt, &["push", "-u", "origin", &format!("{branch}:{branch}")], &ctx.git_env)?;
            let repo = crate::forges::detect_repo(&wt)?;
            let made = crate::forges::create_change_request(&repo, &wt, &crate::forges::NewChangeRequest { title: &title, body: &body, base: &base, head: &branch })?;
            Ok((format!("MR abierta → {base}: {}", made.url), Some((made.url, made.number))))
        })()
    } else {
        merge_local(Path::new(&project.project_path), &base, &branch, &format!("Merge {title}"), &ctx.git_env).map(|_| (format!("fusionada en {base} local · sin push"), None))
    };
    let result = result.map(|(what, mr)| {
        if let Some((url, number)) = mr {
            run.mr_url = Some(url);
            run.mr_number = Some(number);
        }
        what
    });
    let conn = lock(ctx.db)?;
    match result {
        Ok(what) => {
            finish_step(&conn, &step, "done", Some(&what), None, 0.0)?;
            activity(&conn, &run.id, Some(&step), "ent", "cmd", &what, None)?;
            run.status = if project.delivery == "mr" { "delivered" } else { "merged" }.into();
            run.stage = "done".into();
            run.finished_at = Some(Utc::now().to_rfc3339());
            save_run(&conn, run)?;
            drop(conn);
            (ctx.notify)(&format!("{} entregada", order.key), &what);
        }
        Err(e) => {
            finish_step(&conn, &step, "failed", Some(&e), None, 0.0)?;
            run.status = "waiting_human".into();
            run.state.human_kind = Some("merge".into());
            run.state.human = Some(format!("No se pudo entregar: {e}"));
            save_run(&conn, run)?;
            drop(conn);
            (ctx.notify)(&format!("✋ {}: la entrega espera", order.key), &e);
        }
    }
    (ctx.changed)();
    Ok(())
}

/// What a person decided on a run that waits for one.
pub fn resolve(conn: &Connection, run_id: &str, action: &str, note: Option<String>, account_id: Option<String>) -> Result<Run, String> {
    let mut run = load_run(conn, run_id)?;
    let note = note.filter(|n| !n.trim().is_empty());
    match action {
        // Go on from where it is: ✋ approved, credits topped up, merge retried.
        "continue" | "retry" => {}
        "reject_spec" => {
            run.stage = "spec".into();
            run.state.note = note.clone();
        }
        "send_back" => {
            run.round += 1;
            run.state.feedback = Some(note.clone().unwrap_or_else(|| "Una persona devolvió la orden a Fabricación.".into()));
            run.state.gate_ok = false;
            run.stage = "build".into();
        }
        // The approver is the one that failed and the suite is green: a
        // person takes its place and the order goes to delivery.
        "skip_approver" => {
            if run.stage != "approve" || !run.state.gate_ok {
                return Err("solo se puede entregar sin aprobador con la suite en verde, en la etapa del aprobador".into());
            }
            run.stage = "deliver".into();
            run.state.approver = Some(serde_json::json!({ "decision": "ship", "reason": "una persona la entregó sin aprobador" }));
            activity(conn, &run.id, None, "dec", "warn", "entregada sin aprobador por decisión de una persona", None)?;
        }
        "other_account" => {
            let account = account_id.ok_or("elige la cuenta")?;
            let agent = current_agent(conn, &run)?;
            run.state.accounts.insert(agent, account);
        }
        other => return Err(format!("acción desconocida `{other}`")),
    }
    run.status = "running".into();
    run.resume_at = None;
    run.state.human = None;
    run.state.human_kind = None;
    if run.finished_at.is_some() && (action == "retry" || action == "skip_approver") {
        run.finished_at = None;
        run.error = None;
        refresh_for_retry(conn, &mut run)?;
    }
    save_run(conn, &run)?;
    Ok(run)
}

/// A failed order is retried with the team and the brain as they are now:
/// what made it fail (an approver on a model its account lacks, a broken
/// check) is fixed in Equipo or in the Brain, not in the run.
pub fn refresh_for_retry(conn: &Connection, run: &mut Run) -> Result<(), String> {
    let team = super::team::load(conn, &run.project_id)?;
    if !team.is_empty() {
        run.team = team;
    }
    let sealed: Option<i64> = conn.query_row("SELECT brain_version FROM factory_projects WHERE project_id = ?1", params![run.project_id], |r| r.get(0)).ok().flatten();
    if let Some(v) = sealed.filter(|v| *v > run.brain_version) {
        run.brain_version = v;
        conn.execute("UPDATE factory_runs SET brain_version = ?2 WHERE id = ?1", params![run.id, v]).map_err(|e| e.to_string())?;
        conn.execute("UPDATE factory_orders SET brain_version = ?2 WHERE id = ?1", params![run.order_id, v]).map_err(|e| e.to_string())?;
        // The checks filed as broken were fixed in this version: they run again.
        run.state.broken.clear();
        activity(conn, &run.id, None, "ing", "brain", &format!("sigue con el brain v{v}"), None)?;
    }
    Ok(())
}

/// The brain rules a gate command checks. A tester's command isn't the
/// brain's: it can't be waived or filed against the brain (`fix_where` says
/// where it's fixed instead).
fn brain_rules_of(conn: &Connection, run: &Run, r: &GateResult, fix_where: &str) -> Result<Vec<String>, String> {
    let known: std::collections::HashSet<String> = {
        let mut stmt = conn.prepare("SELECT rule_id FROM factory_brain_rules WHERE project_id = ?1 AND version = ?2").map_err(|e| e.to_string())?;
        let rows = stmt.query_map(params![run.project_id, run.brain_version], |x| x.get::<_, String>(0)).map_err(|e| e.to_string())?;
        rows.filter_map(Result::ok).collect()
    };
    let names: Vec<String> = r.name.split(", ").map(str::to_string).collect();
    match names.iter().find(|n| !known.contains(*n)) {
        Some(tester) => Err(format!("{tester} es un tester, no una regla del brain: {fix_where}")),
        None => Ok(names),
    }
}

/// What a person does with one check of the gate: `fix` sends it to the
/// builder, `broken` files it as an amendment to the brain, `waive` stops
/// running it for this order. `check` is the command, as the gate ran it.
pub fn resolve_check(conn: &Connection, run_id: &str, action: &str, check: Option<&str>, note: Option<String>) -> Result<Run, String> {
    let mut run = load_run(conn, run_id)?;
    // While the line works on it, its worker owns the run's state.
    if run.status == "running" {
        return Err("la orden está en marcha: espera a que se pare o páusala".into());
    }
    let order = orders::get(conn, &run.order_id)?;
    let note = note.filter(|n| !n.trim().is_empty());
    let red: Vec<GateResult> = run.state.gate.iter().filter(|g| !g.ok && check.is_none_or(|c| g.command == c)).cloned().collect();
    if red.is_empty() {
        return Err("ese check ya no está en rojo".into());
    }
    match action {
        "fix" => {
            let mut why = String::from("Una persona te pide que arregles estos checks de la puerta:\n\n");
            why.push_str(&red.iter().map(|r| format!("✗ {} ({})\n{}", r.name, r.command, r.tail)).collect::<Vec<_>>().join("\n\n"));
            if let Some(n) = &note {
                why.push_str(&format!("\n\nNota: {n}"));
            }
            activity(conn, &run.id, None, "fab", "warn", &format!("↺ una persona la devuelve a Fabricación: {}", red.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", ")), None)?;
            if run.finished_at.is_some() {
                // A failed order gets its rounds afresh, with today's team.
                refresh_for_retry(conn, &mut run)?;
                run.round = 0;
                run.state.escalated = false;
                run.finished_at = None;
                run.error = None;
            }
            run.round += 1;
            run.state.feedback = Some(why);
            run.state.gate_ok = false;
            run.stage = "build".into();
            run.status = "running".into();
            run.resume_at = None;
            run.state.human = None;
            run.state.human_kind = None;
        }
        "broken" => {
            let r = &red[0];
            let rules = brain_rules_of(conn, &run, r, "cámbialo en Equipo")?;
            let question = format!("El check de {} no funciona: `{}`", r.name, r.command);
            let context = format!("{}\n{}", note.clone().unwrap_or_default(), r.tail).trim().to_string();
            super::brain::add_amendment(conn, &run.project_id, &question, Some(&order.key), Some(&context))?;
            for id in rules {
                if !run.state.broken.contains(&id) {
                    run.state.broken.push(id);
                }
            }
            activity(conn, &run.id, None, "test", "warn", &format!("check de {} → bandeja del brain; no corre hasta que lo arregles y selles", r.name), None)?;
        }
        "waive" => {
            let r = &red[0];
            let rules = brain_rules_of(conn, &run, r, "arréglalo en Fabricación o apágalo en Equipo")?;
            for id in &rules {
                if !run.state.waived.contains(id) {
                    run.state.waived.push(id.clone());
                }
            }
            run.state.warnings.retain(|w| !w.starts_with("Checks ignorados por una persona:"));
            run.state.warnings.push(format!("Checks ignorados por una persona: {}", run.state.waived.join(", ")));
            activity(conn, &run.id, None, "test", "warn", &format!("{} ignorado en esta orden por una persona", rules.join(", ")), None)?;
        }
        other => return Err(format!("acción desconocida `{other}`")),
    }
    save_run(conn, &run)?;
    Ok(run)
}

/// The agent the run is stuck on: the one of its last step.
fn current_agent(conn: &Connection, run: &Run) -> Result<String, String> {
    conn.query_row("SELECT agent_id FROM factory_steps WHERE run_id = ?1 AND agent_id IS NOT NULL ORDER BY started_at DESC LIMIT 1", params![run.id], |r| r.get(0))
        .map_err(|_| "no hay ningún agente al que cambiar la cuenta".to_string())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::factory::agent::{Activity, AgentResult};
    use std::sync::Mutex;

    /// A scripted CLI: per role, a list of answers taken in order.
    pub(crate) struct Script {
        pub answers: Mutex<HashMap<String, Vec<AgentResult>>>,
        pub calls: Mutex<Vec<(String, Option<String>, String)>>,
    }

    impl Script {
        pub(crate) fn new() -> Self {
            Script { answers: Mutex::new(HashMap::new()), calls: Mutex::new(Vec::new()) }
        }
        pub(crate) fn on(&self, role: &str, json: Value) {
            self.answers.lock().unwrap().entry(role.into()).or_default().push(AgentResult { outcome: Outcome::Ok, json: Some(json), session_id: Some(format!("s-{role}")), cost_usd: 0.1 });
        }
        pub(crate) fn fail(&self, role: &str) {
            self.answers.lock().unwrap().entry(role.into()).or_default().push(AgentResult { outcome: Outcome::Failed("boom".into()), json: None, session_id: None, cost_usd: 0.0 });
        }
        pub(crate) fn broke(&self, role: &str) {
            self.answers.lock().unwrap().entry(role.into()).or_default().push(AgentResult { outcome: Outcome::NoCredits, json: None, session_id: None, cost_usd: 0.0 });
        }
        pub(crate) fn limit(&self, role: &str) {
            self.answers.lock().unwrap().entry(role.into()).or_default().push(AgentResult {
                outcome: Outcome::Limit { resume_at: Utc::now() + chrono::Duration::minutes(14) },
                json: None,
                session_id: Some(format!("s-{role}")),
                cost_usd: 0.0,
            });
        }
    }

    pub(crate) fn role_of(prompt: &str) -> String {
        for (role, marker) in [
            ("spec", "Escribe la spec"),
            ("critic", "Revisa la spec contra"),
            ("builder", "Implementa la spec"),
            ("tester", "Escribe los tests"),
            ("reviewer_code", "Revisa el diff de la rama"),
            ("reviewer_security", "ojos de seguridad"),
            ("reviewer_ui", "Revisa la interfaz"),
            ("reviewer_ux", "Revisa los flujos"),
            ("verifier", "Te dan hallazgos"),
            ("approver", "Los tests están en verde"),
        ] {
            if prompt.contains(marker) {
                return role.into();
            }
        }
        "?".into()
    }

    impl Runner for Script {
        fn run(&self, call: &AgentCall, on: &mut dyn FnMut(Activity)) -> AgentResult {
            let role = role_of(&call.prompt);
            self.calls.lock().unwrap().push((role.clone(), call.resume.clone(), call.model.clone()));
            on(Activity { kind: "read", text: format!("{role} lee el repo") });
            if role == "builder" {
                let f = call.cwd.join("app.txt");
                let n = std::fs::read_to_string(&f).unwrap_or_default();
                std::fs::write(&f, format!("{n}x\n")).unwrap();
                let _ = git(&call.cwd, &["add", "-A"], &[]);
                let _ = git(&call.cwd, &["commit", "-m", "build"], &crate::factory::setup::tests_identity());
            }
            let mut answers = self.answers.lock().unwrap();
            let list = answers.entry(role.clone()).or_default();
            if list.is_empty() {
                return AgentResult { outcome: Outcome::Ok, json: Some(json!({"summary": format!("{role} ok"), "decisions": [], "findings": [], "warnings": [], "tests": [], "ok": true, "issues": [], "revised_summary": "", "confirmed": [], "dismissed": [], "decision": "ship", "reason": "verde"})), session_id: Some(format!("s-{role}")), cost_usd: 0.1 };
            }
            list.remove(0)
        }
    }

    pub(crate) struct World {
        pub db: DbConnection,
        pub repo: PathBuf,
        pub order_id: String,
        _gate: Gate,
    }

    /// Every test world is project `p1`, and ⏹ is per project and global: a
    /// test that presses it must not stop the others running beside it. They
    /// share this gate; the one that stops takes it alone.
    static STOP_GATE: std::sync::RwLock<()> = std::sync::RwLock::new(());

    #[allow(dead_code)]
    enum Gate {
        Shared(std::sync::RwLockReadGuard<'static, ()>),
        Alone(std::sync::RwLockWriteGuard<'static, ()>),
    }

    /// A world whose test presses ⏹: it waits for the others to finish.
    pub(crate) fn world_alone(mode: &str, delivery: &str, tester_cmd: &str) -> World {
        let gate = Gate::Alone(STOP_GATE.write().unwrap_or_else(|e| e.into_inner()));
        build_world(mode, delivery, tester_cmd, gate)
    }

    /// A real git repo (main + dev), the Citas Vet brain sealed, the default
    /// team, and VET-2 in the queue.
    pub(crate) fn world(mode: &str, delivery: &str, tester_cmd: &str) -> World {
        let gate = Gate::Shared(STOP_GATE.read().unwrap_or_else(|e| e.into_inner()));
        build_world(mode, delivery, tester_cmd, gate)
    }

    fn build_world(mode: &str, delivery: &str, tester_cmd: &str, gate: Gate) -> World {
        let conn = crate::factory::team::tests::conn_with_accounts(&[("work", "claude"), ("cx", "codex")]);
        let repo = std::env::temp_dir().join(format!("hangar-line-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&repo).unwrap();
        crate::factory::setup::init_repo(&repo, &crate::factory::setup::tests_identity()).unwrap();
        conn.execute("UPDATE projects SET path = ?1 WHERE id = 'p1'", params![repo.to_string_lossy()]).unwrap();
        conn.execute("UPDATE factory_projects SET mode = ?1, delivery = ?2 WHERE project_id = 'p1'", params![mode, delivery]).unwrap();
        // The example brain without its checks, which need Python tools.
        let md: String = brain::EXAMPLE.lines().filter(|l| !l.trim_start().starts_with("check:")).collect::<Vec<_>>().join("\n");
        // App data beside the repo, not in it: the repo must stay the project's.
        brain::seal(&conn, &repo.with_extension("appdata"), "p1", "Citas Vet", &md, "import").unwrap();
        let mut team = crate::factory::team::load(&conn, "p1").unwrap();
        for a in team.iter_mut().filter(|a| a.role == "tester") {
            a.command = Some(tester_cmd.into());
        }
        crate::factory::team::save(&conn, "p1", &team).unwrap();
        let order_id = orders::insert(
            &conn,
            "p1",
            &orders::NewOrder { key: "VET-2".into(), title: "Cancelar una cita".into(), criteria: vec!["Marca cancelada_at".into()], origin: "trd".into(), ..Default::default() },
        )
        .unwrap();
        // A ⏹ left over from another test doesn't belong to this world.
        crate::factory::procs::clear_stop("p1");
        World { db: DbConnection(Mutex::new(conn)), repo, order_id, _gate: gate }
    }

    fn ctx<'a>(w: &'a World, runner: &'a dyn Runner) -> Ctx<'a> {
        static NOTIFY: fn(&str, &str) = |_, _| {};
        static CHANGED: fn() = || {};
        Ctx { db: &w.db, runner, git_env: crate::factory::setup::tests_identity(), root: None, notify: &NOTIFY, changed: &CHANGED }
    }

    fn roles(s: &Script) -> Vec<String> {
        s.calls.lock().unwrap().iter().map(|c| c.0.clone()).collect()
    }

    #[test]
    fn the_citas_vet_order_goes_from_spec_to_a_local_merge() {
        let w = world("autonomous", "local", "true");
        let s = Script::new();
        // Security finds SEC-01 the first time; the verifier confirms it; one
        // round back to Fabricación; then it's clean.
        s.on("reviewer_security", json!({"summary":"","warnings":[],"findings":[{"rule_id":"SEC-01","severity":"blocker","file":"app/repos/citas.py","text":"no comprueba que la cita sea del cliente"}],"decisions":[{"rule_id":"SEC-01","text":"exige validar el dueño"}]}));
        s.on("verifier", json!({"confirmed":[0],"dismissed":[],"decisions":[]}));
        s.on("spec", json!({"summary":"cancelar = borrado lógico","steps":[],"files":[],"tests":[],"risk":false,"risk_reason":"","decisions":[
            {"rule_id":"DAT-03","text":"borrado lógico con cancelada_at"},
            {"rule_id":null,"text":"no se avisa al veterinario","question":"¿Se avisa al veterinario cuando se cancela una cita?"}]}));
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        assert!(run.branch.starts_with("feat/vet-2-cancelar-una-cita"));
        let run = advance(&c, &run.id).unwrap();
        assert_eq!(run.status, "merged", "{:?}", run.error);
        assert_eq!(run.round, 1, "one round for SEC-01");
        assert_eq!(
            roles(&s),
            vec!["spec", "critic", "builder", "tester", "tester", "reviewer_code", "reviewer_security", "verifier", "builder", "tester", "tester", "reviewer_code", "reviewer_security", "approver"]
        );
        let conn = w.db.0.lock().unwrap();
        assert_eq!(orders::get(&conn, &w.order_id).unwrap().status, "merged");
        let inbox = brain::amendments(&conn, "p1").unwrap();
        assert_eq!(inbox[0].question, "¿Se avisa al veterinario cuando se cancela una cita?");
        let cited: i64 = conn.query_row("SELECT COUNT(*) FROM factory_decisions WHERE run_id = ?1 AND rule_id IS NOT NULL", params![run.id], |r| r.get(0)).unwrap();
        assert_eq!(cited, 2);
        assert!(git(&w.repo, &["log", "dev", "--oneline"], &[]).unwrap().contains("Merge VET-2"), "merged into dev, which isn't checked out");
        assert!(git(&w.repo, &["remote"], &[]).unwrap().trim().is_empty(), "nothing to push to, nothing pushed");
    }

    #[test]
    fn the_gate_is_never_skipped_red_goes_back_and_the_builder_escalates() {
        let w = world("autonomous", "local", "false");
        let s = Script::new();
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let run = advance(&c, &run.id).unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(run.round, MAX_ROUNDS + 1);
        let calls = s.calls.lock().unwrap().clone();
        assert!(!calls.iter().any(|c| c.0 == "approver" || c.0.starts_with("reviewer")), "red never reaches review or the approver");
        let builder_models: Vec<String> = calls.iter().filter(|c| c.0 == "builder").map(|c| c.2.clone()).collect();
        assert_eq!(builder_models.len(), 5);
        assert_eq!(&builder_models[..3], &["sonnet", "sonnet", "sonnet"]);
        assert_eq!(&builder_models[3..], &["opus", "opus"], "escalated after K rounds");
        let conn = w.db.0.lock().unwrap();
        assert_eq!(orders::get(&conn, &w.order_id).unwrap().status, "failed");
    }

    #[test]
    fn a_limit_waits_in_the_same_conversation_and_never_counts_as_a_failure() {
        let w = world("autonomous", "local", "true");
        let s = Script::new();
        s.limit("builder");
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let run = advance(&c, &run.id).unwrap();
        assert_eq!((run.status.as_str(), run.stage.as_str()), ("waiting_limit", "build"));
        assert!(run.resume_at.is_some());
        assert!(run.error.is_none());
        {
            let conn = w.db.0.lock().unwrap();
            assert_eq!(orders::get(&conn, &w.order_id).unwrap().status, "waiting_limit");
        }
        let run = advance(&c, &run.id).unwrap();
        assert_eq!(run.status, "merged");
        let calls = s.calls.lock().unwrap().clone();
        let builders: Vec<&(String, Option<String>, String)> = calls.iter().filter(|c| c.0 == "builder").collect();
        assert_eq!(builders[1].1.as_deref(), Some("s-builder"), "resumed with --resume, same conversation");
    }

    #[test]
    fn supervised_waits_for_you_on_the_spec_and_the_diff() {
        let w = world("supervised", "local", "true");
        let s = Script::new();
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let run = advance(&c, &run.id).unwrap();
        assert_eq!((run.status.as_str(), run.state.human_kind.as_deref()), ("waiting_human", Some("spec_review")));
        {
            let conn = w.db.0.lock().unwrap();
            resolve(&conn, &run.id, "continue", None, None).unwrap();
        }
        let run = advance(&c, &run.id).unwrap();
        assert_eq!(run.state.human_kind.as_deref(), Some("diff"));
        {
            let conn = w.db.0.lock().unwrap();
            resolve(&conn, &run.id, "continue", None, None).unwrap();
        }
        assert_eq!(advance(&c, &run.id).unwrap().status, "merged");
    }

    fn approver_off(w: &World) {
        let conn = w.db.0.lock().unwrap();
        let mut team = crate::factory::team::load(&conn, "p1").unwrap();
        team.iter_mut().find(|a| a.role == "approver").unwrap().active = false;
        crate::factory::team::save(&conn, "p1", &team).unwrap();
    }

    #[test]
    fn without_an_approver_the_gate_still_decides_and_delegated_stops_on_risk() {
        // One world at a time: each holds the stop gate, and a second taken
        // while a ⏹ test waits for it never comes.
        {
            let w = world("autonomous", "local", "true");
            approver_off(&w);
            let s = Script::new();
            let c = ctx(&w, &s);
            let run = start(&c, "p1", &w.order_id).unwrap();
            let run = advance(&c, &run.id).unwrap();
            assert_eq!(run.status, "merged", "{:?}", run.error);
            assert!(!roles(&s).contains(&"approver".to_string()));
            assert!(roles(&s).contains(&"tester".to_string()), "the gate's suite still ran");
        }

        let w = world("delegated", "local", "true");
        approver_off(&w);
        let s = Script::new();
        s.on("spec", json!({"summary":"x","steps":[],"files":[],"tests":[],"risk":true,"risk_reason":"toca pagos","decisions":[]}));
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let run = advance(&c, &run.id).unwrap();
        assert_eq!(run.state.human_kind.as_deref(), Some("spec_review"), "risky: the spec waits first, as ever");
        {
            let conn = w.db.0.lock().unwrap();
            resolve(&conn, &run.id, "continue", None, None).unwrap();
        }
        let run = advance(&c, &run.id).unwrap();
        assert_eq!((run.status.as_str(), run.state.human_kind.as_deref()), ("waiting_human", Some("approver")));
        assert!(run.state.human.as_deref().unwrap().contains("toca pagos"));
    }

    #[test]
    fn a_failed_approver_is_skipped_by_a_person_or_retried_with_todays_team() {
        // «Entregar sin aprobador»: straight to delivery, the suite green.
        // One world at a time (see above).
        {
            let w = world("autonomous", "local", "true");
            let s = Script::new();
            s.fail("approver");
            let c = ctx(&w, &s);
            let run = start(&c, "p1", &w.order_id).unwrap();
            let run = advance(&c, &run.id).unwrap();
            assert_eq!((run.status.as_str(), run.stage.as_str()), ("failed", "approve"));
            {
                let conn = w.db.0.lock().unwrap();
                resolve(&conn, &run.id, "skip_approver", None, None).unwrap();
            }
            assert_eq!(advance(&c, &run.id).unwrap().status, "merged");
            assert_eq!(roles(&s).iter().filter(|r| *r == "approver").count(), 1, "not asked again");
        }

        // Switched off in Equipo, then «Reintentar»: the run takes the team as it is now.
        let w = world("autonomous", "local", "true");
        let s = Script::new();
        s.fail("approver");
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let run = advance(&c, &run.id).unwrap();
        assert_eq!(run.status, "failed");
        approver_off(&w);
        {
            let conn = w.db.0.lock().unwrap();
            let run = resolve(&conn, &run.id, "retry", None, None).unwrap();
            assert!(!run.team.iter().any(|a| a.role == "approver" && a.active));
        }
        assert_eq!(advance(&c, &run.id).unwrap().status, "merged");
        assert_eq!(roles(&s).iter().filter(|r| *r == "approver").count(), 1);
    }

    #[test]
    fn skipping_the_approver_needs_a_green_suite_at_its_stage() {
        let w = world("autonomous", "local", "true");
        let s = Script::new();
        s.fail("spec");
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let run = advance(&c, &run.id).unwrap();
        let conn = w.db.0.lock().unwrap();
        assert!(resolve(&conn, &run.id, "skip_approver", None, None).is_err());
    }

    #[test]
    fn ui_and_ux_that_cannot_open_the_app_warn_and_the_order_goes_on() {
        let w = world("autonomous", "local", "true");
        {
            let conn = w.db.0.lock().unwrap();
            conn.execute("UPDATE factory_orders SET touches_ui = 1 WHERE id = ?1", params![w.order_id]).unwrap();
        }
        let s = Script::new();
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let run = advance(&c, &run.id).unwrap();
        assert_eq!(run.status, "merged");
        assert!(run.state.warnings.iter().any(|w| w.contains("no se pudo abrir la app")), "{:?}", run.state.warnings);
        let r = roles(&s);
        assert!(r.contains(&"reviewer_ui".to_string()) && r.contains(&"reviewer_ux".to_string()));
    }

    #[test]
    fn merging_locally_where_the_base_is_checked_out_refuses_uncommitted_work() {
        let w = world("autonomous", "local", "true");
        git(&w.repo, &["checkout", "dev"], &[]).unwrap();
        git(&w.repo, &["checkout", "-b", "feat/x"], &[]).unwrap();
        std::fs::write(w.repo.join("x.txt"), "x").unwrap();
        git(&w.repo, &["add", "-A"], &[]).unwrap();
        git(&w.repo, &["commit", "-m", "x"], &crate::factory::setup::tests_identity()).unwrap();
        git(&w.repo, &["checkout", "dev"], &[]).unwrap();
        std::fs::write(w.repo.join("README.md"), "dirty").unwrap();
        git(&w.repo, &["add", "README.md"], &[]).unwrap();
        assert!(merge_local(&w.repo, "dev", "feat/x", "Merge x", &crate::factory::setup::tests_identity()).unwrap_err().contains("sin commit"));
        git(&w.repo, &["reset", "--hard"], &[]).unwrap();
        merge_local(&w.repo, "dev", "feat/x", "Merge x", &crate::factory::setup::tests_identity()).unwrap();
        assert!(w.repo.join("x.txt").exists());
    }

    #[test]
    fn the_gate_runs_the_orders_checks_once_each_and_order_zero_only_asks_them_to_run() {
        let w = world("autonomous", "local", "true");
        let conn = w.db.0.lock().unwrap();
        let team = crate::factory::team::load(&conn, "p1").unwrap();
        let mut brain = brain::load(&conn, "p1", None).unwrap().unwrap();
        drop(conn);
        for r in brain.rules.iter_mut() {
            r.check = match r.id.as_str() {
                "ARQ-01" | "ARQ-02" => Some("echo capas".into()),
                "SEC-01" => Some("exit 3".into()),
                "SEC-02" => Some("definitely-missing-xyz".into()),
                _ => None,
            };
        }
        let mine: std::collections::HashSet<String> = ["ARQ-01", "ARQ-02"].iter().map(|s| s.to_string()).collect();
        let cmds = gate_commands(&team, &brain, &mine, false);
        assert_eq!(cmds.iter().filter(|(_, c)| c == "echo capas").count(), 1, "one run for two rules");
        assert!(cmds.iter().any(|(n, c)| c == "echo capas" && n == "ARQ-01, ARQ-02"));
        assert!(!cmds.iter().any(|(_, c)| c == "exit 3"), "SEC-01 isn't this order's");
        assert!(cmds.iter().filter(|(_, c)| c == "true").count() == 1, "both testers run `true`: once");

        let all = gate_commands(&team, &brain, &Default::default(), true);
        let runs = run_gate("p1", &w.repo, &all, GateMode::Runs);
        let by = |c: &str| runs.iter().find(|r| r.command == c).unwrap().ok;
        assert!(by("exit 3"), "order 0: it ran, failing is fine");
        assert!(!by("definitely-missing-xyz"), "order 0: not runnable is red");
        let green = run_gate("p1", &w.repo, &all, GateMode::Green);
        assert!(!green.iter().find(|r| r.command == "exit 3").unwrap().ok);
    }

    #[test]
    fn order_zero_sets_up_without_testers_and_passes_if_every_command_runs() {
        // `false` runs (and fails): fine for order 0, which only sets things up.
        let w = world("autonomous", "local", "false");
        {
            let conn = w.db.0.lock().unwrap();
            conn.execute("UPDATE factory_orders SET origin = 'bootstrap' WHERE id = ?1", params![w.order_id]).unwrap();
        }
        let s = Script::new();
        let c = ctx(&w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let run = advance(&c, &run.id).unwrap();
        assert_eq!(run.status, "merged", "{:?}", run.error);
        assert!(!roles(&s).contains(&"tester".to_string()), "no testers in order 0");
    }

    /// A notification is written to the database too: sent while the line
    /// holds the lock, the same thread waits on itself and the whole app
    /// freezes — which is what happened when an order hit a usage limit.
    #[test]
    fn notifications_are_sent_without_the_database_lock_held() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for case in ["limit", "fail", "credits", "rounds"] {
            let w = world("autonomous", "local", if case == "rounds" { "false" } else { "true" });
            let s = Script::new();
            match case {
                "limit" => s.limit("spec"),
                "fail" => s.fail("spec"),
                "credits" => s.broke("spec"),
                _ => {}
            }
            let sent = AtomicUsize::new(0);
            let notify = |_: &str, _: &str| {
                assert!(w.db.0.try_lock().is_ok(), "{case}: notified with the database lock held");
                sent.fetch_add(1, Ordering::SeqCst);
            };
            static C: fn() = || {};
            let ctx = Ctx { db: &w.db, runner: &s, git_env: crate::factory::setup::tests_identity(), root: None, notify: &notify, changed: &C };
            let run = start(&ctx, "p1", &w.order_id).unwrap();
            advance(&ctx, &run.id).unwrap();
            assert!(sent.load(Ordering::SeqCst) >= 1, "{case}: the user was told");
        }
    }

    #[test]
    fn branch_names_follow_git_01() {
        let o = Order {
            id: "1".into(),
            key: "#12".into(),
            title: "Cancelar una cita (DELETE /citas/{id})".into(),
            description: String::new(),
            criteria: vec![],
            deps: vec![],
            position: 0,
            status: "todo".into(),
            origin: "trd".into(),
            ext_id: None,
            ext_url: None,
            touches_ui: false,
            rules: vec![],
            brain_version: None,
            created_at: String::new(),
            blocked_by: vec![],
        };
        assert_eq!(branch_name(&o), "feat/12-cancelar-una-cita-delete-citas-id");
        let pok = Order { key: "POK-1".into(), title: "Inicializar repo".into(), ..o };
        assert_eq!(branch_name(&pok), "feat/pok-1-inicializar-repo", "GIT-04 wants `[a-z0-9-]`");
    }

    #[test]
    fn a_red_command_shows_what_failed_not_just_how_it_ended() {
        let mut out = String::from("$ playwright test\n[WebServer] $ vite build\n");
        out.push_str("  ✘  12 [firefox] › e2e/csp.spec.ts:8:3 › / serves the CSP without violations (1.2s)\n");
        for i in 0..60 {
            out.push_str(&format!("[WebServer] dist/assets/chunk-{i}.js  1.2 kB\n"));
        }
        out.push_str("  4 failed\n    [firefox] › e2e/bootstrap.spec.ts:59:1 › CA9: no external fonts\n  31 passed (5.3s)\n ELIFECYCLE  Command failed with exit code 1.\n");
        let ex = failure_excerpt(&out);
        assert!(ex.contains("✘  12 [firefox] › e2e/csp.spec.ts"), "{ex}");
        assert!(ex.contains("4 failed") && ex.contains("CA9: no external fonts"), "{ex}");
        assert!(ex.lines().count() < 30);
    }

    #[test]
    fn waived_checks_leave_the_gate_but_a_shared_command_still_runs() {
        let cmds = vec![("Tester unitario, CAL-01".to_string(), "pnpm test".to_string()), ("GIT-04".to_string(), "git branch".to_string()), ("RSG-09, RSG-10".to_string(), "diff".to_string())];
        let kept = without_waived(cmds, &["GIT-04".into(), "CAL-01".into(), "RSG-09".into()]);
        assert_eq!(kept, vec![("Tester unitario".to_string(), "pnpm test".to_string()), ("RSG-10".to_string(), "diff".to_string())]);
    }

    /// A failed run with these red checks, as the gate leaves it.
    fn failed_with(w: &World, red: &[(&str, &str)]) -> Run {
        let s = Script::new();
        let c = ctx(w, &s);
        let run = start(&c, "p1", &w.order_id).unwrap();
        let conn = w.db.0.lock().unwrap();
        let mut run = load_run(&conn, &run.id).unwrap();
        run.state.gate = red.iter().map(|(n, cmd)| GateResult { name: n.to_string(), command: cmd.to_string(), ok: false, tail: format!("{cmd}: rojo"), rule: None, reds: 4 }).collect();
        run.stage = "gate".into();
        run.round = MAX_ROUNDS + 1;
        run.status = "failed".into();
        run.finished_at = Some(Utc::now().to_rfc3339());
        save_run(&conn, &run).unwrap();
        run
    }

    #[test]
    fn a_red_check_goes_to_fabricacion_the_brain_or_is_waived() {
        let w = world("autonomous", "local", "true");
        let rule: String = w.db.0.lock().unwrap().query_row("SELECT rule_id FROM factory_brain_rules WHERE project_id = 'p1' LIMIT 1", [], |r| r.get(0)).unwrap();
        let run = failed_with(&w, &[("Tester unitario", "pnpm e2e"), (&rule, "git check-ignore -q a b")]);
        let conn = w.db.0.lock().unwrap();

        // A tester's command is the suite: it can't be waived nor blamed on the brain.
        assert!(resolve_check(&conn, &run.id, "waive", Some("pnpm e2e"), None).unwrap_err().contains("es un tester"));
        assert!(resolve_check(&conn, &run.id, "broken", Some("pnpm e2e"), None).is_err());

        let r = resolve_check(&conn, &run.id, "broken", Some("git check-ignore -q a b"), Some("solo admite una ruta".into())).unwrap();
        assert_eq!(r.state.broken, vec![rule.clone()]);
        let inbox = brain::amendments(&conn, "p1").unwrap();
        assert!(inbox[0].question.contains("git check-ignore -q a b"));
        assert_eq!(r.status, "failed", "filing it doesn't restart the order");

        let r = resolve_check(&conn, &run.id, "waive", Some("git check-ignore -q a b"), None).unwrap();
        assert_eq!(r.state.waived, vec![rule.clone()]);
        assert!(r.state.warnings.iter().any(|w| w == &format!("Checks ignorados por una persona: {rule}")));

        let r = resolve_check(&conn, &run.id, "fix", Some("pnpm e2e"), Some("mira Firefox".into())).unwrap();
        assert_eq!((r.status.as_str(), r.stage.as_str(), r.round), ("running", "build", 1), "a failed order gets its rounds afresh");
        let fb = r.state.feedback.unwrap();
        assert!(fb.contains("pnpm e2e: rojo") && fb.contains("mira Firefox") && !fb.contains("check-ignore"), "{fb}");
        assert!(r.finished_at.is_none() && r.error.is_none());
    }

    #[test]
    fn a_retry_goes_on_with_the_newest_brain_and_its_fixed_checks_run_again() {
        let w = world("autonomous", "local", "true");
        let rule: String = w.db.0.lock().unwrap().query_row("SELECT rule_id FROM factory_brain_rules WHERE project_id = 'p1' LIMIT 1", [], |r| r.get(0)).unwrap();
        let run = failed_with(&w, &[(&rule, "exit 1")]);
        let conn = w.db.0.lock().unwrap();
        resolve_check(&conn, &run.id, "broken", None, None).unwrap();
        let md: String = brain::EXAMPLE.lines().filter(|l| !l.trim_start().starts_with("check:")).collect::<Vec<_>>().join("\n");
        brain::seal(&conn, &w.repo.with_extension("appdata"), "p1", "Citas Vet", &md, "import").unwrap();
        let r = resolve(&conn, &run.id, "retry", None, None).unwrap();
        assert_eq!(r.brain_version, run.brain_version + 1);
        assert!(r.state.broken.is_empty(), "fixed in the new brain: it runs again");
        assert_eq!(orders::get(&conn, &w.order_id).unwrap().brain_version, Some(r.brain_version));
    }
}
