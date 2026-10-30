//! The team: every agent on the line with its account, model and the brain
//! sections it reads. Each order freezes the team it started with, so a change
//! here applies from the next order on.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::DbConnection;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    pub id: String,
    /// `spec`, `critic`, `builder`, `tester`, `reviewer_code`,
    /// `reviewer_security`, `reviewer_ui`, `reviewer_ux`, `verifier` or `approver`.
    pub role: String,
    /// Ingeniería, Fabricación, Testers, Revisores or Decisión.
    pub group: String,
    pub name: String,
    pub short: String,
    /// What it does, in a few words.
    pub duty: String,
    /// The brain sections it reads (`arq`, `cal`…); empty reads the rule cited.
    pub sections: Vec<String>,
    pub account_id: Option<String>,
    pub model: String,
    pub active: bool,
    pub required: bool,
    /// Testers: the command that runs their part of the suite.
    pub command: Option<String>,
    /// Builder: the model it moves to after failing twice in a row.
    pub escalate_model: Option<String>,
    pub note: Option<String>,
    pub position: i64,
}

pub const ROLES: [&str; 10] = ["spec", "critic", "builder", "tester", "reviewer_code", "reviewer_security", "reviewer_ui", "reviewer_ux", "verifier", "approver"];

/// The prototype's team. `slot` says which account it gets: `work` is the
/// first Claude account, `personal` the second (or the first again), `other`
/// the first account of another model family, for the approver.
struct Seed {
    role: &'static str,
    group: &'static str,
    name: &'static str,
    short: &'static str,
    duty: &'static str,
    sections: &'static [&'static str],
    slot: &'static str,
    model: &'static str,
    active: bool,
    required: bool,
    command: Option<&'static str>,
    escalate: Option<&'static str>,
    note: Option<&'static str>,
}

const SEEDS: [Seed; 12] = [
    Seed { role: "spec", group: "Ingeniería", name: "Spec writer", short: "spec", duty: "TRD + TDD + rama", sections: &["arq", "prd", "rsg"], slot: "work", model: "opus", active: true, required: true, command: None, escalate: None, note: None },
    Seed { role: "critic", group: "Ingeniería", name: "Crítico R0", short: "crítico", duty: "reescribe si rompe una regla", sections: &["prd", "arq"], slot: "personal", model: "haiku", active: true, required: false, command: None, escalate: None, note: None },
    Seed { role: "builder", group: "Fabricación", name: "Builder", short: "builder", duty: "commits en su worktree", sections: &["cod", "tec", "git"], slot: "work", model: "sonnet", active: true, required: true, command: None, escalate: Some("opus"), note: None },
    Seed { role: "tester", group: "Testers", name: "Tester unitario", short: "unitario", duty: "cada criterio → un test", sections: &["cal"], slot: "work", model: "sonnet", active: true, required: false, command: Some("pytest -q"), escalate: None, note: None },
    Seed { role: "tester", group: "Testers", name: "Tester de integración", short: "integración", duty: "API + base de datos", sections: &["cal", "dat"], slot: "work", model: "sonnet", active: true, required: false, command: Some("pytest -m integration"), escalate: None, note: None },
    Seed { role: "tester", group: "Testers", name: "Tester E2E", short: "e2e", duty: "flujos completos", sections: &["cal"], slot: "personal", model: "sonnet", active: false, required: false, command: Some("npx playwright test"), escalate: None, note: Some("cuando haya interfaz") },
    Seed { role: "reviewer_code", group: "Revisores", name: "Revisor de código", short: "código", duty: "errores, legibilidad", sections: &["cod", "arq"], slot: "work", model: "opus", active: true, required: false, command: None, escalate: None, note: None },
    Seed { role: "reviewer_security", group: "Revisores", name: "Seguridad", short: "seguridad", duty: "entradas, secretos, auth", sections: &["sec"], slot: "work", model: "opus", active: true, required: false, command: None, escalate: None, note: None },
    Seed { role: "reviewer_ui", group: "Revisores", name: "Revisor UI", short: "UI", duty: "capturas vs diseño", sections: &["prd", "cod"], slot: "personal", model: "sonnet", active: true, required: false, command: None, escalate: None, note: Some("solo si toca la interfaz") },
    Seed { role: "reviewer_ux", group: "Revisores", name: "Revisor UX", short: "UX", duty: "flujos y textos", sections: &["prd"], slot: "personal", model: "sonnet", active: true, required: false, command: None, escalate: None, note: Some("solo si toca la interfaz") },
    Seed { role: "verifier", group: "Decisión", name: "Verificador", short: "verificador", duty: "¿el hallazgo es real?", sections: &[], slot: "personal", model: "haiku", active: true, required: false, command: None, escalate: None, note: None },
    Seed { role: "approver", group: "Decisión", name: "Aprobador", short: "aprobador", duty: "ship o te pregunta", sections: &["rsg", "git"], slot: "other", model: "gpt-5", active: true, required: true, command: None, escalate: None, note: Some("otra familia de modelo") },
];

/// The testers «+ Añadir tester» offers, in order.
pub const EXTRA_TESTERS: [(&str, &str, &str, &str); 3] = [
    ("Tester de rendimiento", "rendimiento", "tiempos de respuesta", "locust --headless -t 1m"),
    ("Tester de contrato", "contrato", "la API cumple su OpenAPI", "schemathesis run openapi.json"),
    ("Tester de mutación", "mutación", "¿los tests detectan fallos?", "mutmut run"),
];

/// Which company's models an account runs — the approver should be from a
/// different one than the builder, so it doesn't agree with itself.
pub fn family(provider: &str) -> &str {
    match provider {
        "claude" => "anthropic",
        "codex" => "openai",
        "gemini" => "google",
        other => other,
    }
}

/// The models an account can run. Claude's CLI takes aliases; Codex has no
/// catalog Hangar can read, so it's the prototype's list.
pub fn models_for(provider: &str, default_model: Option<&str>) -> Vec<String> {
    match provider {
        "claude" => crate::providers::claude::known_model_aliases().into_iter().map(|m| m.id).collect(),
        "codex" => vec!["gpt-5".into(), "gpt-5-codex".into(), "gpt-5-mini".into()],
        _ => default_model.map(|m| vec![m.to_string()]).unwrap_or_default(),
    }
}

/// `(id, provider, default_model)` of every account, oldest first.
fn accounts(conn: &Connection) -> Result<Vec<(String, String, Option<String>)>, String> {
    let mut stmt = conn.prepare("SELECT id, provider, default_model FROM accounts ORDER BY created_at").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// The prototype's team on this machine's accounts.
pub fn default_team(conn: &Connection) -> Result<Vec<Agent>, String> {
    let all = accounts(conn)?;
    let claude: Vec<&(String, String, Option<String>)> = all.iter().filter(|a| a.1 == "claude").collect();
    let work = claude.first().copied().or(all.first());
    let personal = claude.get(1).copied().or(work);
    let other = all.iter().find(|a| work.is_some_and(|w| family(&a.1) != family(&w.1))).or(work);
    Ok(SEEDS
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let account = match s.slot {
                "work" => work,
                "personal" => personal,
                _ => other,
            };
            // A model this account can't run falls back to its first one.
            let model = match account {
                Some((_, provider, default)) => {
                    let models = models_for(provider, default.as_deref());
                    if models.iter().any(|m| m == s.model) || models.is_empty() { s.model.to_string() } else { models[0].clone() }
                }
                None => s.model.to_string(),
            };
            Agent {
                id: uuid::Uuid::new_v4().to_string(),
                role: s.role.into(),
                group: s.group.into(),
                name: s.name.into(),
                short: s.short.into(),
                duty: s.duty.into(),
                sections: s.sections.iter().map(|x| x.to_string()).collect(),
                account_id: account.map(|a| a.0.clone()),
                model,
                active: s.active,
                required: s.required,
                command: s.command.map(Into::into),
                escalate_model: s.escalate.map(Into::into),
                note: s.note.map(Into::into),
                position: i as i64,
            }
        })
        .collect())
}

fn from_row(r: &rusqlite::Row) -> rusqlite::Result<Agent> {
    let sections: String = r.get(6)?;
    Ok(Agent {
        id: r.get(0)?,
        role: r.get(1)?,
        group: r.get(2)?,
        name: r.get(3)?,
        short: r.get(4)?,
        duty: r.get(5)?,
        sections: serde_json::from_str(&sections).unwrap_or_default(),
        account_id: r.get(7)?,
        model: r.get(8)?,
        active: r.get(9)?,
        required: r.get(10)?,
        command: r.get(11)?,
        escalate_model: r.get(12)?,
        note: r.get(13)?,
        position: r.get(14)?,
    })
}

/// The project's team, seeded with the default one the first time.
pub fn load(conn: &Connection, project_id: &str) -> Result<Vec<Agent>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, role, grp, name, short, duty, sections, account_id, model, active, required, command, escalate_model, note, position
               FROM factory_agents WHERE project_id = ?1 ORDER BY position",
        )
        .map_err(|e| e.to_string())?;
    let team: Vec<Agent> = stmt.query_map(params![project_id], from_row).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    if !team.is_empty() {
        return Ok(team);
    }
    let seeded = default_team(conn)?;
    save(conn, project_id, &seeded)?;
    Ok(seeded)
}

/// Replaces the team. Required agents stay on whatever the list says.
pub fn save(conn: &Connection, project_id: &str, team: &[Agent]) -> Result<(), String> {
    for a in team {
        if !ROLES.contains(&a.role.as_str()) {
            return Err(format!("rol desconocido `{}`", a.role));
        }
        if a.role == "tester" && a.active && a.command.as_deref().map(str::trim).unwrap_or("").is_empty() {
            return Err(format!("{}: un tester necesita el comando que corre su parte de la suite", a.name));
        }
    }
    conn.execute("DELETE FROM factory_agents WHERE project_id = ?1", params![project_id]).map_err(|e| e.to_string())?;
    for (i, a) in team.iter().enumerate() {
        conn.execute(
            "INSERT INTO factory_agents (id, project_id, role, grp, name, short, duty, sections, account_id, model, active, required, command, escalate_model, note, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                a.id,
                project_id,
                a.role,
                a.group,
                a.name,
                a.short,
                a.duty,
                serde_json::to_string(&a.sections).unwrap_or_else(|_| "[]".into()),
                a.account_id,
                a.model,
                a.active || a.required,
                a.required,
                a.command.as_deref().map(str::trim).filter(|c| !c.is_empty()),
                a.escalate_model,
                a.note,
                i as i64
            ],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Whether the line can run with this team: `problems` block, `warnings` don't.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TeamCheck {
    pub problems: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn check(conn: &Connection, team: &[Agent]) -> Result<TeamCheck, String> {
    let providers: std::collections::HashMap<String, String> = accounts(conn)?.into_iter().map(|(id, p, _)| (id, p)).collect();
    let mut out = TeamCheck::default();
    let on = |role: &str| team.iter().find(|a| a.role == role && (a.active || a.required));
    for (role, label) in [("spec", "spec writer"), ("builder", "builder"), ("approver", "aprobador")] {
        match on(role) {
            None => out.problems.push(format!("Falta el {label}: es obligatorio.")),
            Some(a) if a.account_id.as_deref().map(|id| !providers.contains_key(id)).unwrap_or(true) => out.problems.push(format!("{}: elige una cuenta.", a.name)),
            _ => {}
        }
    }
    if !team.iter().any(|a| a.role == "tester" && a.active) {
        out.problems.push("Activa al menos un tester: sin tests no se aprueba nada.".into());
    }
    for a in team.iter().filter(|a| a.active && !a.required) {
        if a.account_id.as_deref().map(|id| !providers.contains_key(id)).unwrap_or(true) {
            out.problems.push(format!("{}: elige una cuenta.", a.name));
        }
    }
    let fam = |a: &Agent| a.account_id.as_ref().and_then(|id| providers.get(id)).map(|p| family(p).to_string());
    if let (Some(b), Some(ap)) = (on("builder"), on("approver")) {
        if fam(b).is_some() && fam(b) == fam(ap) {
            out.warnings.push("El aprobador es de la misma familia de modelo que el builder: tiende a darle la razón. Mejor otra familia (p. ej. Codex).".into());
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- commands

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamView {
    pub agents: Vec<Agent>,
    pub check: TeamCheck,
}

#[tauri::command]
pub fn factory_team_get(db: State<DbConnection>, project_id: String) -> Result<TeamView, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let agents = load(&conn, &project_id)?;
    let check = check(&conn, &agents)?;
    Ok(TeamView { agents, check })
}

#[tauri::command]
pub fn factory_team_save(db: State<DbConnection>, project_id: String, agents: Vec<Agent>) -> Result<TeamView, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    save(&conn, &project_id, &agents)?;
    let agents = load(&conn, &project_id)?;
    let check = check(&conn, &agents)?;
    Ok(TeamView { agents, check })
}

/// Account id → the models it can run, for the team's selects.
#[tauri::command]
pub fn factory_models(db: State<DbConnection>) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(accounts(&conn)?.into_iter().map(|(id, provider, default)| (id, models_for(&provider, default.as_deref()))).collect())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn conn_with_accounts(accounts: &[(&str, &str)]) -> Connection {
        let conn = crate::factory::store::tests::conn();
        conn.execute_batch("CREATE TABLE accounts (id TEXT PRIMARY KEY, provider TEXT, default_model TEXT, created_at TEXT);").unwrap();
        for (i, (id, provider)) in accounts.iter().enumerate() {
            conn.execute("INSERT INTO accounts VALUES (?1, ?2, NULL, ?3)", params![id, provider, i.to_string()]).unwrap();
        }
        crate::factory::store::add(&conn, "p1", "local").unwrap();
        conn
    }

    #[test]
    fn the_default_team_is_the_prototypes_on_real_accounts() {
        let c = conn_with_accounts(&[("work", "claude"), ("personal", "claude"), ("cx", "codex")]);
        let team = load(&c, "p1").unwrap();
        assert_eq!(team.len(), 12);
        let by = |name: &str| team.iter().find(|a| a.name == name).unwrap();
        assert_eq!((by("Spec writer").account_id.as_deref(), by("Spec writer").model.as_str()), (Some("work"), "opus"));
        assert_eq!(by("Crítico R0").account_id.as_deref(), Some("personal"));
        assert_eq!((by("Aprobador").account_id.as_deref(), by("Aprobador").model.as_str()), (Some("cx"), "gpt-5"));
        assert_eq!(by("Builder").escalate_model.as_deref(), Some("opus"));
        assert!(!by("Tester E2E").active);
        let check = check(&c, &team).unwrap();
        assert!(check.problems.is_empty() && check.warnings.is_empty(), "{check:?}");
        assert_eq!(load(&c, "p1").unwrap(), team, "seeded once, then read back");
    }

    #[test]
    fn without_testers_the_line_cannot_run() {
        let c = conn_with_accounts(&[("work", "claude"), ("cx", "codex")]);
        let mut team = load(&c, "p1").unwrap();
        for a in team.iter_mut().filter(|a| a.role == "tester") {
            a.active = false;
        }
        save(&c, "p1", &team).unwrap();
        let check = check(&c, &load(&c, "p1").unwrap()).unwrap();
        assert!(check.problems.iter().any(|p| p.contains("al menos un tester")));
    }

    #[test]
    fn an_approver_of_the_builders_family_is_warned_about() {
        let c = conn_with_accounts(&[("work", "claude")]);
        let team = load(&c, "p1").unwrap();
        let approver = team.iter().find(|a| a.role == "approver").unwrap();
        assert_eq!((approver.account_id.as_deref(), approver.model.as_str()), (Some("work"), "opus"), "no other family: claude, with a model it runs");
        let check = check(&c, &team).unwrap();
        assert!(check.problems.is_empty());
        assert_eq!(check.warnings.len(), 1);
    }

    #[test]
    fn required_agents_stay_on_and_a_tester_needs_its_command() {
        let c = conn_with_accounts(&[("work", "claude"), ("cx", "codex")]);
        let mut team = load(&c, "p1").unwrap();
        team.iter_mut().find(|a| a.role == "builder").unwrap().active = false;
        save(&c, "p1", &team).unwrap();
        assert!(load(&c, "p1").unwrap().iter().find(|a| a.role == "builder").unwrap().active);
        team.iter_mut().find(|a| a.name == "Tester unitario").unwrap().command = Some("  ".into());
        assert!(save(&c, "p1", &team).is_err());
    }
}
