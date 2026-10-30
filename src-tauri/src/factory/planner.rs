//! Input B: a TRD broken down into orders by the planner — the spec agent,
//! reading the TRD, the sealed brain and the repo. You review the breakdown
//! (✋, always); approved, the orders are created in the provider or kept in
//! Hangar, and they become the queue.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime, State};

use super::agent::{AgentCall, CliRunner, Outcome, Runner};
use super::orders::{self, NewOrder, Order};
use crate::db::DbConnection;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PlannedOrder {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub criteria: Vec<String>,
    /// 1-based positions of the orders this one waits for.
    #[serde(default)]
    pub deps: Vec<usize>,
    /// The TRD section it comes from (`§1`).
    #[serde(default)]
    pub trd_part: String,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub touches_ui: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub part: String,
    pub orders: Vec<usize>,
    pub rules: Vec<String>,
}

/// The breakdown you review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub orders: Vec<PlannedOrder>,
    #[serde(default)]
    pub coverage: Vec<Coverage>,
    #[serde(default)]
    pub risks: Vec<String>,
    /// Parts of the TRD no order covers.
    #[serde(default)]
    pub uncovered: Vec<String>,
}

pub fn schema() -> Value {
    json!({
        "type": "object",
        "required": ["orders", "coverage", "risks", "uncovered"],
        "properties": {
            "orders": { "type": "array", "items": {
                "type": "object",
                "required": ["title", "description", "criteria", "deps", "trd_part", "rules", "touches_ui"],
                "properties": {
                    "title": { "type": "string" },
                    "description": { "type": "string" },
                    "criteria": { "type": "array", "items": { "type": "string" } },
                    "deps": { "type": "array", "items": { "type": "integer" } },
                    "trd_part": { "type": "string" },
                    "rules": { "type": "array", "items": { "type": "string" } },
                    "touches_ui": { "type": "boolean" }
                }
            }},
            "coverage": { "type": "array", "items": {
                "type": "object", "required": ["part", "orders", "rules"],
                "properties": { "part": { "type": "string" }, "orders": { "type": "array", "items": { "type": "integer" } }, "rules": { "type": "array", "items": { "type": "string" } } }
            }},
            "risks": { "type": "array", "items": { "type": "string" } },
            "uncovered": { "type": "array", "items": { "type": "string" } }
        }
    })
}

pub fn prompt(trd: &str, brain: &str) -> String {
    format!(
        "Eres el planificador de la Factory. Divide este TRD en órdenes de trabajo para una línea automática que las hará de una en una.\n\
Lee el repo para saber qué existe ya. Decide SIEMPRE con el brain sellado de abajo: cada orden lista las reglas (IDs) que le aplican, \
y respeta el tamaño máximo de una orden que diga la sección Riesgo.\n\
Para cada orden: título corto, descripción, criterios de aceptación comprobables (cada uno será un test), dependencias (números 1-based \
de otras órdenes de esta lista), la parte del TRD de la que sale (§N) y si toca la interfaz de usuario.\n\
Después: la cobertura (cada parte del TRD → sus órdenes y reglas), los riesgos, y lo que el TRD pide y ninguna orden cubre.\n\
No escribas ningún archivo.\n\n=== BRAIN ===\n{brain}\n\n=== TRD ===\n{trd}"
    )
}

/// Reads the planner's answer, dropping dependencies that point nowhere.
pub fn read_plan(answer: &Value) -> Result<Plan, String> {
    // The schema says snake_case; the struct speaks camelCase to the UI.
    let mut v = answer.clone();
    if let Some(list) = v.get_mut("orders").and_then(Value::as_array_mut) {
        for o in list {
            if let Some(obj) = o.as_object_mut() {
                for (from, to) in [("trd_part", "trdPart"), ("touches_ui", "touchesUi")] {
                    if let Some(x) = obj.remove(from) {
                        obj.insert(to.into(), x);
                    }
                }
            }
        }
    }
    let mut plan: Plan = serde_json::from_value(v).map_err(|e| format!("el planificador respondió algo ilegible: {e}"))?;
    let n = plan.orders.len();
    for (i, o) in plan.orders.iter_mut().enumerate() {
        o.deps.retain(|d| *d >= 1 && *d <= n && *d != i + 1);
    }
    if plan.orders.is_empty() {
        return Err("el planificador no sacó ninguna orden del TRD".into());
    }
    Ok(plan)
}

/// Saves an approved plan as the project's next orders: `<PREFIX>-N` keys,
/// dependencies as keys. Returns the new orders' ids, in plan order.
pub fn save_plan(conn: &rusqlite::Connection, project_id: &str, prefix: &str, plan: &Plan) -> Result<Vec<String>, String> {
    let first: i64 = orders::next_key(conn, project_id, prefix)?.rsplit('-').next().and_then(|n| n.parse().ok()).unwrap_or(1);
    let keys: Vec<String> = (0..plan.orders.len() as i64).map(|i| format!("{prefix}-{}", first + i)).collect();
    let mut ids = Vec::new();
    for (i, o) in plan.orders.iter().enumerate() {
        let deps = o.deps.iter().map(|d| keys[d - 1].clone()).collect();
        let mut description = o.description.clone();
        if !o.trd_part.is_empty() {
            description.push_str(&format!("\n\nTRD {}", o.trd_part));
        }
        ids.push(orders::insert(
            conn,
            project_id,
            &NewOrder {
                key: keys[i].clone(),
                title: o.title.clone(),
                description,
                criteria: o.criteria.clone(),
                deps,
                origin: "trd".into(),
                touches_ui: o.touches_ui,
                rules: o.rules.clone(),
                ..Default::default()
            },
        )?);
    }
    Ok(ids)
}

/// The spec agent: who plans and writes specs.
fn spec_agent(conn: &rusqlite::Connection, project_id: &str) -> Result<(String, String), String> {
    let team = super::team::load(conn, project_id)?;
    let spec = team.iter().find(|a| a.role == "spec").ok_or("el equipo no tiene spec writer")?;
    Ok((spec.account_id.clone().ok_or("el spec writer no tiene cuenta")?, spec.model.clone()))
}

pub const TRD_FILE: &str = ".hangar/trd/TRD.md";

// ---------------------------------------------------------------- commands

/// 🧩 Analizar: the planner reads the TRD, the brain and the repo.
#[tauri::command]
pub async fn factory_plan_trd<R: Runtime>(app: AppHandle<R>, project_id: String, trd: String) -> Result<Plan, String> {
    crate::blocking::run_blocking(move || {
        let root = crate::app_data::root(&app).ok();
        let db = app.state::<DbConnection>();
        let (project, brain, account, model, runner) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let project = super::store::get(&conn, &project_id)?;
            let brain = super::brain::load(&conn, &project_id, None)?.ok_or("primero sella el brain: el planificador decide con él")?;
            let (account, model) = spec_agent(&conn, &project_id)?;
            let runner = CliRunner::for_accounts(&conn, std::slice::from_ref(&account), root)?;
            (project, brain, account, model, runner)
        };
        let call = AgentCall {
            key: format!("plan-{project_id}"),
            account_id: account,
            model,
            cwd: project.project_path.clone().into(),
            prompt: prompt(&trd, &brain.markdown),
            schema: schema(),
            resume: None,
            mcp_config: None,
            write: false,
            timeout: Duration::from_secs(20 * 60),
        };
        let result = runner.run(&call, &mut |_| {});
        match result.outcome {
            Outcome::Ok => read_plan(result.json.as_ref().ok_or("el planificador no devolvió el desglose")?),
            Outcome::Limit { resume_at } => Err(format!("la cuenta del spec writer llegó a su límite; se renueva a las {}", resume_at.with_timezone(&chrono::Local).format("%H:%M"))),
            Outcome::NoCredits => Err("la cuenta del spec writer no tiene créditos".into()),
            Outcome::Stopped => Err("análisis cancelado".into()),
            Outcome::Failed(why) => Err(format!("el planificador falló: {why}")),
        }
    })
    .await?
}

/// Aprobar: the orders go into the queue and, with a provider, are created in
/// the project's list or repo too.
#[tauri::command]
pub async fn factory_plan_approve<R: Runtime>(app: AppHandle<R>, project_id: String, plan: Plan) -> Result<Vec<Order>, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (project, ids) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let project = super::store::get(&conn, &project_id)?;
            let ids = save_plan(&conn, &project_id, &orders::key_prefix(&project.project_name), &plan)?;
            (project, ids)
        };
        export_orders(&db, &project, &ids)?;
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let all = orders::list(&conn, &project_id)?;
        Ok(all.into_iter().filter(|o| ids.contains(&o.id)).collect())
    })
    .await?
}

/// Creates the given orders in the project's provider, if it has one.
fn export_orders(db: &DbConnection, project: &super::store::FactoryProject, ids: &[String]) -> Result<usize, String> {
    let (Some(source), Some(scope)) = (&project.provider_source_id, &project.provider_scope_id) else { return Ok(0) };
    if project.provider_kind == "none" {
        return Ok(0);
    }
    let mut created = 0;
    for id in ids {
        let order = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            orders::get(&conn, id)?
        };
        let (ext_id, url) = crate::task_sources::create_in_scope(db, source, scope, &format!("{} · {}", order.key, order.title), &orders::task_body(&order))?;
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        orders::set_external(&conn, id, &ext_id, &url)?;
        created += 1;
    }
    Ok(created)
}

/// «Conectar un proveedor más adelante»: the local orders nobody started go
/// to the provider just chosen, and live there from now on.
#[tauri::command]
pub async fn factory_orders_export<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<usize, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let (project, ids) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let project = super::store::get(&conn, &project_id)?;
            if project.provider_kind == "none" {
                return Err("elige antes un proveedor en el paso Proyecto".to_string());
            }
            let ids: Vec<String> = orders::unstarted_local(&conn, &project_id)?.into_iter().map(|o| o.id).collect();
            (project, ids)
        };
        export_orders(&db, &project, &ids)
    })
    .await?
}

/// Input A: the open tasks of the project's list or repo, into the queue.
#[tauri::command]
pub async fn factory_orders_sync<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<usize, String> {
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let project = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            super::store::get(&conn, &project_id)?
        };
        let (Some(source), Some(scope)) = (&project.provider_source_id, &project.provider_scope_id) else { return Ok(0) };
        let tasks = crate::task_sources::fetch_scope(&db, source, scope, Path::new(&project.project_path))?;
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        orders::sync_provider(&conn, &project_id, &tasks)
    })
    .await?
}

#[tauri::command]
pub fn factory_orders_list(db: State<DbConnection>, project_id: String) -> Result<Vec<Order>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    orders::list(&conn, &project_id)
}

#[tauri::command]
pub fn factory_orders_reorder(db: State<DbConnection>, project_id: String, ids: Vec<String>) -> Result<Vec<Order>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    orders::reorder(&conn, &project_id, &ids)?;
    orders::list(&conn, &project_id)
}

/// An order typed by hand, for a project without a provider.
#[tauri::command]
pub fn factory_order_add(db: State<DbConnection>, project_id: String, title: String, criteria: Vec<String>, deps: Vec<String>) -> Result<Vec<Order>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let project = super::store::get(&conn, &project_id)?;
    if title.trim().is_empty() {
        return Err("la orden necesita un título".into());
    }
    let key = orders::next_key(&conn, &project_id, &orders::key_prefix(&project.project_name))?;
    let criteria = criteria.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
    orders::insert(&conn, &project_id, &NewOrder { key, title: title.trim().into(), criteria, deps, origin: "manual".into(), ..Default::default() })?;
    orders::list(&conn, &project_id)
}

/// «No me convence»: the TRD goes into the project as a file and a terminal
/// opens to refine it with the AI; «Volver a analizar» reads it back.
#[tauri::command]
pub async fn factory_trd_refine<R: Runtime>(app: AppHandle<R>, project_id: String, trd: String, objection: String) -> Result<crate::terminals::TerminalInfo, String> {
    crate::blocking::run_blocking(move || {
        let (project, account) = {
            let db = app.state::<DbConnection>();
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            (super::store::get(&conn, &project_id)?, spec_agent(&conn, &project_id)?.0)
        };
        let dir = Path::new(&project.project_path);
        let file = dir.join(TRD_FILE);
        std::fs::create_dir_all(file.parent().expect("has a parent")).map_err(|e| e.to_string())?;
        std::fs::write(&file, &trd).map_err(|e| format!("no se pudo escribir {}: {e}", file.display()))?;
        crate::task_sources::exclude_hangar_dir_locally(dir);
        let mut prompt = format!(
            "Vamos a afinar juntos el TRD que está en {TRD_FILE}: el desglose en órdenes que salió de él no me convenció. \
Pregúntame lo que haga falta y, cuando lo acordemos, edita ese archivo. Respeta el brain del proyecto (.hangar/brain o el que te diga)."
        );
        if !objection.trim().is_empty() {
            prompt.push_str(&format!("\n\nLo que no me convenció: {}", objection.trim()));
        }
        crate::terminals::open_terminal_blocking(
            app,
            crate::terminals::TerminalTarget { kind: crate::terminals::TargetKind::Project, id: project_id },
            Some(account),
            Some(crate::terminals::InitCommand::Start),
            None,
            Some(prompt),
            100,
            30,
        )
    })
    .await?
}

#[tauri::command]
pub fn factory_trd_read(db: State<DbConnection>, project_id: String) -> Result<Option<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let project = super::store::get(&conn, &project_id)?;
    Ok(std::fs::read_to_string(Path::new(&project.project_path).join(TRD_FILE)).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Citas Vet breakdown, as the prototype shows it.
    pub(crate) fn citas_vet_answer() -> Value {
        json!({
            "orders": [
                {"title":"Reservar cita (POST /citas)","description":"","criteria":["Crea la cita","Rechaza solapes"],"deps":[],"trd_part":"§1","rules":["ARQ-02","DAT-01"],"touches_ui":false},
                {"title":"Cancelar cita (borrado lógico)","description":"","criteria":["Marca cancelada_at"],"deps":[1],"trd_part":"§2","rules":["DAT-03","SEC-01"],"touches_ui":false},
                {"title":"Agenda: franjas libres","description":"","criteria":["Lista libres"],"deps":[1],"trd_part":"§3","rules":["ARQ-01"],"touches_ui":false},
                {"title":"Agenda: franjas ocupadas","description":"","criteria":["Lista ocupadas"],"deps":[3],"trd_part":"§3","rules":["ARQ-01"],"touches_ui":false},
                {"title":"Recordatorio: job 24 h antes","description":"","criteria":["Envía 24 h antes"],"deps":[1],"trd_part":"§4","rules":["SEC-02"],"touches_ui":false},
                {"title":"Recordatorio: plantilla email","description":"","criteria":["Plantilla configurable"],"deps":[5, 9, 6],"trd_part":"§4","rules":["SEC-02"],"touches_ui":false}
            ],
            "coverage": [{"part":"§1 Reservar","orders":[1],"rules":["ARQ-02","DAT-01"]}],
            "risks": ["#1 crea la tabla citas → DAT-01, migración obligatoria"],
            "uncovered": []
        })
    }

    #[test]
    fn the_citas_vet_trd_gives_six_orders_with_dependencies_in_the_queue() {
        let plan = read_plan(&citas_vet_answer()).unwrap();
        assert_eq!(plan.orders.len(), 6);
        assert_eq!(plan.orders[5].deps, vec![5], "a dependency on itself or on nothing is dropped");
        assert_eq!(plan.orders[0].trd_part, "§1");

        let c = crate::factory::store::tests::conn();
        crate::factory::store::add(&c, "p1", "local").unwrap();
        save_plan(&c, "p1", "VET", &plan).unwrap();
        let queue = orders::list(&c, "p1").unwrap();
        assert_eq!(queue.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(), vec!["VET-1", "VET-2", "VET-3", "VET-4", "VET-5", "VET-6"]);
        assert_eq!(queue[3].deps, vec!["VET-3"]);
        assert_eq!(queue[1].rules, vec!["DAT-03", "SEC-01"]);
        assert_eq!(orders::next(&c, "p1").unwrap().unwrap().key, "VET-1");
        assert_eq!(queue.iter().filter(|o| !o.blocked_by.is_empty()).count(), 5);
    }

    #[test]
    fn an_empty_breakdown_is_refused() {
        assert!(read_plan(&json!({"orders": [], "coverage": [], "risks": [], "uncovered": []})).is_err());
    }
}
