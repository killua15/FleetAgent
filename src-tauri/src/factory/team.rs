//! The team: every agent on the line with its account, model and the brain
//! sections it reads. Each order freezes the team it started with, so a change
//! here applies from the next order on.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};




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

pub const ROLES: [&str; 11] = ["spec", "critic", "designer", "builder", "tester", "reviewer_code", "reviewer_security", "reviewer_ui", "reviewer_ux", "verifier", "approver"];

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
    command: Option<&'static str>,
    escalate: Option<&'static str>,
    note: Option<&'static str>,
}

const SEEDS: [Seed; 13] = [
    Seed { role: "spec", group: "Ingeniería", name: "Spec writer", short: "spec", duty: "TRD + TDD + rama", sections: &["arq", "prd", "rsg"], slot: "work", model: "opus", active: true, command: None, escalate: None, note: None },
    Seed { role: "critic", group: "Ingeniería", name: "Crítico R0", short: "crítico", duty: "reescribe si rompe una regla", sections: &["prd", "arq"], slot: "personal", model: "haiku", active: true, command: None, escalate: None, note: None },
    Seed { role: "designer", group: "Ingeniería", name: "Diseñador", short: "diseño", duty: "pantallas, estados y adaptación", sections: &["prd", "dis"], slot: "work", model: "opus", active: true, command: None, escalate: None, note: Some("solo si toca la interfaz") },
    Seed { role: "builder", group: "Fabricación", name: "Builder", short: "builder", duty: "commits en su worktree", sections: &["cod", "tec", "git"], slot: "work", model: "sonnet", active: true, command: None, escalate: Some("opus"), note: None },
    Seed { role: "tester", group: "Testers", name: "Tester unitario", short: "unitario", duty: "cada criterio → un test", sections: &["cal"], slot: "work", model: "sonnet", active: true, command: Some("pytest -q"), escalate: None, note: None },
    Seed { role: "tester", group: "Testers", name: "Tester de integración", short: "integración", duty: "API + base de datos", sections: &["cal", "dat"], slot: "work", model: "sonnet", active: true, command: Some("pytest -m integration"), escalate: None, note: None },
    Seed { role: "tester", group: "Testers", name: "Tester E2E", short: "e2e", duty: "flujos completos", sections: &["cal"], slot: "personal", model: "sonnet", active: false, command: Some("npx playwright test"), escalate: None, note: Some("cuando haya interfaz") },
    Seed { role: "reviewer_code", group: "Revisores", name: "Revisor de código", short: "código", duty: "errores, legibilidad", sections: &["cod", "arq"], slot: "work", model: "opus", active: true, command: None, escalate: None, note: None },
    Seed { role: "reviewer_security", group: "Revisores", name: "Seguridad", short: "seguridad", duty: "entradas, secretos, auth", sections: &["sec"], slot: "work", model: "opus", active: true, command: None, escalate: None, note: None },
    Seed { role: "reviewer_ui", group: "Revisores", name: "Revisor UI", short: "UI", duty: "capturas vs diseño", sections: &["prd", "dis", "cod"], slot: "personal", model: "sonnet", active: true, command: None, escalate: None, note: Some("solo si toca la interfaz") },
    Seed { role: "reviewer_ux", group: "Revisores", name: "Revisor UX", short: "UX", duty: "flujos y textos", sections: &["prd"], slot: "personal", model: "sonnet", active: true, command: None, escalate: None, note: Some("solo si toca la interfaz") },
    Seed { role: "verifier", group: "Decisión", name: "Verificador", short: "verificador", duty: "¿el hallazgo es real?", sections: &[], slot: "personal", model: "haiku", active: true, command: None, escalate: None, note: None },
    Seed { role: "approver", group: "Decisión", name: "Aprobador", short: "aprobador", duty: "ship o te pregunta", sections: &["rsg", "git"], slot: "other", model: "gpt-5", active: true, command: None, escalate: None, note: Some("otra familia de modelo") },
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

/// The only agents the line can't run without. The approver is not one: a
/// team on a single model family has nobody to give a second opinion.
pub fn is_required(role: &str) -> bool {
    matches!(role, "spec" | "builder")
}

/// The models an account can run. Claude's CLI takes aliases; Codex keeps
/// the catalog of what this login may use in its config folder, and a model
/// outside it (a ChatGPT account on `gpt-5`) fails every turn.
pub fn models_for(provider: &str, default_model: Option<&str>, config_dir: Option<&str>) -> Vec<String> {
    match provider {
        "claude" => crate::providers::claude::known_model_aliases().into_iter().map(|m| m.id).collect(),
        "codex" => {
            let dir = config_dir.map(super::setup::expand_home);
            let listed = dir.as_deref().map(codex_catalog).unwrap_or_default();
            if !listed.is_empty() {
                return listed;
            }
            // No catalog yet (never opened): the model its config picks.
            match dir.as_deref().and_then(codex_config_model).or_else(|| default_model.map(str::to_string)) {
                Some(m) => vec![m],
                None => vec!["gpt-5-codex".into()],
            }
        }
        _ => default_model.map(|m| vec![m.to_string()]).unwrap_or_default(),
    }
}

/// The models Codex lists for this login (`models_cache.json`), as its own
/// picker shows them.
fn codex_catalog(dir: &std::path::Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("models_cache.json")) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    v.get("models")
        .and_then(serde_json::Value::as_array)
        .map(|ms| {
            ms.iter()
                .filter(|m| m.get("visibility").and_then(serde_json::Value::as_str) == Some("list"))
                .filter_map(|m| m.get("slug").and_then(serde_json::Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// `model = "…"` in the account's `config.toml`.
fn codex_config_model(dir: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("config.toml")).ok()?;
    let line = text.lines().take_while(|l| !l.trim_start().starts_with('[')).find(|l| l.trim_start().starts_with("model ") || l.trim_start().starts_with("model="))?;
    let value = line.split_once('=')?.1.trim().trim_matches('"');
    (!value.is_empty()).then(|| value.to_string())
}

/// One account as the team sees it: `(id, provider, the models it runs)`,
/// oldest first.
fn accounts(conn: &Connection) -> Result<Vec<(String, String, Vec<String>)>, String> {
    let mut stmt = conn.prepare("SELECT id, provider, default_model, config_dir_path FROM accounts ORDER BY created_at").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            let (provider, default, dir): (String, Option<String>, Option<String>) = (r.get(1)?, r.get(2)?, r.get(3)?);
            let models = models_for(&provider, default.as_deref(), dir.as_deref());
            Ok((r.get(0)?, provider, models))
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// The prototype's team on this machine's accounts.
pub fn default_team(conn: &Connection) -> Result<Vec<Agent>, String> {
    let all = accounts(conn)?;
    let claude: Vec<&(String, String, Vec<String>)> = all.iter().filter(|a| a.1 == "claude").collect();
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
                Some((_, _, models)) => {
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
                required: is_required(s.role),
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
        // By role, not as stored: a team saved when the approver was
        // obligatory can switch it off now.
        required: is_required(&r.get::<_, String>(1)?),
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
    let mut team: Vec<Agent> = stmt.query_map(params![project_id], from_row).map_err(|e| e.to_string())?.collect::<Result<_, _>>().map_err(|e| e.to_string())?;
    if !team.is_empty() {
        // A team saved before a role existed gets it, from the seed: the
        // designer came after the first projects.
        let missing: Vec<Agent> = default_team(conn)?.into_iter().filter(|a| a.role == "designer" && !team.iter().any(|t| t.role == a.role)).collect();
        if !missing.is_empty() {
            let at = team.iter().position(|a| a.role == "builder").unwrap_or(team.len());
            for (i, a) in missing.into_iter().enumerate() {
                team.insert(at + i, a);
            }
            save(conn, project_id, &team)?;
            return load(conn, project_id);
        }
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
                a.active || is_required(&a.role),
                is_required(&a.role),
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
    let all = accounts(conn)?;
    let providers: std::collections::HashMap<String, String> = all.iter().map(|(id, p, _)| (id.clone(), p.clone())).collect();
    let models: std::collections::HashMap<&str, &Vec<String>> = all.iter().map(|(id, _, m)| (id.as_str(), m)).collect();
    let mut out = TeamCheck::default();
    let on = |role: &str| team.iter().find(|a| a.role == role && (a.active || is_required(&a.role)));
    for (role, label) in [("spec", "spec writer"), ("builder", "builder")] {
        match on(role) {
            None => out.problems.push(format!("Falta el {label}: es obligatorio.")),
            Some(a) if a.account_id.as_deref().map(|id| !providers.contains_key(id)).unwrap_or(true) => out.problems.push(format!("{}: elige una cuenta.", a.name)),
            _ => {}
        }
    }
    if !team.iter().any(|a| a.role == "tester" && a.active) {
        out.problems.push("Activa al menos un tester: sin tests no se aprueba nada.".into());
    }
    for a in team.iter().filter(|a| a.active && !is_required(&a.role)) {
        if a.account_id.as_deref().map(|id| !providers.contains_key(id)).unwrap_or(true) {
            out.problems.push(format!("{}: elige una cuenta.", a.name));
        }
    }
    // A model Codex doesn't offer this login fails on every turn. (Claude's
    // CLI takes ids beyond its aliases, so only Codex is checked.)
    for a in team.iter().filter(|a| a.active || is_required(&a.role)) {
        let codex = a.account_id.as_deref().and_then(|id| providers.get(id)).is_some_and(|p| p == "codex");
        if let Some(list) = a.account_id.as_deref().and_then(|id| models.get(id)).filter(|_| codex) {
            if !list.is_empty() && !list.contains(&a.model) {
                out.problems.push(format!("{}: la cuenta no ofrece el modelo {}; elige otro.", a.name, a.model));
            }
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

// ---------------------------------------------------------------- from the brain

/// A tester whose command the brain says differently.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub agent_id: String,
    pub name: String,
    pub current: Option<String>,
    pub current_active: bool,
    /// `None` with `active: false`: the brain has no such tests, switch it off.
    pub suggested: Option<String>,
    pub active: bool,
}

const RUNNERS: [&str; 14] = ["pnpm", "npm", "yarn", "bun", "npx", "vitest", "jest", "pytest", "cargo", "go", "uv", "poetry", "python", "python3"];

/// How the brain's Calidad section says the project is tested: unit,
/// integration and E2E commands, as far as its `check:` tell.
impl Suggestion {
    /// `unitario` out of «Tester unitario», for a short sentence.
    pub fn short(&self) -> String {
        self.name.strip_prefix("Tester de ").or_else(|| self.name.strip_prefix("Tester ")).unwrap_or(&self.name).to_string()
    }
}

pub fn brain_test_commands(brain: &super::brain::Sealed) -> (Option<String>, Option<String>, Option<String>) {
    let is_runner = |c: &str| super::env::first_program(c).is_some_and(|p| RUNNERS.contains(&p.as_str()));
    let cmds: Vec<&str> = brain.rules.iter().filter(|r| r.section == "cal").filter_map(|r| r.check.as_deref()).filter(|c| is_runner(c)).collect();
    let shortest = |pick: &dyn Fn(&str) -> bool| cmds.iter().copied().filter(|c| pick(c)).min_by_key(|c| c.len()).map(str::to_string);
    let is_e2e = |c: &str| c.contains("e2e") || c.contains("playwright");
    let runs_tests = regex::Regex::new(r"\b(test|vitest|jest|pytest)\b").expect("static regex");
    let e2e = shortest(&|c| is_e2e(c));
    let integration = shortest(&|c| c.contains("integration"));
    let unit = shortest(&|c| !is_e2e(c) && !c.contains("integration") && runs_tests.is_match(c));
    (unit, integration, e2e)
}

/// The testers the brain would set differently.
pub fn suggestions(team: &[Agent], brain: &super::brain::Sealed) -> Vec<Suggestion> {
    let (unit, integration, e2e) = brain_test_commands(brain);
    if unit.is_none() && integration.is_none() && e2e.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for a in team.iter().filter(|a| a.role == "tester") {
        let (suggested, active) = match a.short.as_str() {
            "unitario" => match &unit {
                Some(c) => (Some(c.clone()), true),
                None => continue,
            },
            "integración" => match &integration {
                Some(c) => (Some(c.clone()), true),
                None => (None, false),
            },
            "e2e" => match &e2e {
                Some(c) => (Some(c.clone()), true),
                None => continue,
            },
            _ => continue,
        };
        let same = if active { a.active && a.command.as_deref() == suggested.as_deref() } else { !a.active };
        if !same {
            out.push(Suggestion { agent_id: a.id.clone(), name: a.name.clone(), current: a.command.clone(), current_active: a.active, suggested, active });
        }
    }
    out
}

pub fn apply_suggestions(conn: &Connection, project_id: &str, brain: &super::brain::Sealed) -> Result<usize, String> {
    let mut team = load(conn, project_id)?;
    let changes = suggestions(&team, brain);
    for s in &changes {
        if let Some(a) = team.iter_mut().find(|a| a.id == s.agent_id) {
            a.active = s.active;
            if let Some(c) = &s.suggested {
                a.command = Some(c.clone());
            }
        }
    }
    save(conn, project_id, &team)?;
    Ok(changes.len())
}

// ---------------------------------------------------------------- commands

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamView {
    pub agents: Vec<Agent>,
    pub check: TeamCheck,
}

#[tauri::command]
pub async fn factory_team_get<R: tauri::Runtime>(app: tauri::AppHandle<R>, project_id: String) -> Result<TeamView, String> {
    crate::factory::with_db(app, move |conn| {
        let agents = load(&conn, &project_id)?;
        let check = check(&conn, &agents)?;
        Ok(TeamView { agents, check })
    })
    .await
}

#[tauri::command]
pub async fn factory_team_save<R: tauri::Runtime>(app: tauri::AppHandle<R>, project_id: String, agents: Vec<Agent>) -> Result<TeamView, String> {
    crate::factory::with_db(app, move |conn| {
        save(&conn, &project_id, &agents)?;
        let agents = load(&conn, &project_id)?;
        let check = check(&conn, &agents)?;
        Ok(TeamView { agents, check })
    })
    .await
}

#[tauri::command]
pub async fn factory_team_suggestions<R: tauri::Runtime>(app: tauri::AppHandle<R>, project_id: String) -> Result<Vec<Suggestion>, String> {
    crate::factory::with_db(app, move |conn| {
        let Some(brain) = super::brain::load(&conn, &project_id, None)? else { return Ok(Vec::new()) };
        Ok(suggestions(&load(&conn, &project_id)?, &brain))
    })
    .await
}

/// «Aplicar los del brain».
#[tauri::command]
pub async fn factory_team_apply_suggestions<R: tauri::Runtime>(app: tauri::AppHandle<R>, project_id: String) -> Result<TeamView, String> {
    crate::factory::with_db(app, move |conn| {
        let brain = super::brain::load(&conn, &project_id, None)?.ok_or("todavía no hay un brain sellado")?;
        apply_suggestions(&conn, &project_id, &brain)?;
        let agents = load(&conn, &project_id)?;
        let check = check(&conn, &agents)?;
        Ok(TeamView { agents, check })
    })
    .await
}

/// Account id → the models it can run, for the team's selects.
#[tauri::command]
pub async fn factory_models<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<std::collections::HashMap<String, Vec<String>>, String> {
    crate::factory::with_db(app, move |conn| {
        Ok(accounts(&conn)?.into_iter().map(|(id, _, models)| (id, models)).collect())
    })
    .await
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn conn_with_accounts(accounts: &[(&str, &str)]) -> Connection {
        let conn = crate::factory::store::tests::conn();
        conn.execute_batch("CREATE TABLE accounts (id TEXT PRIMARY KEY, provider TEXT, default_model TEXT, created_at TEXT, config_dir_path TEXT);").unwrap();
        for (i, (id, provider)) in accounts.iter().enumerate() {
            conn.execute("INSERT INTO accounts VALUES (?1, ?2, NULL, ?3, NULL)", params![id, provider, i.to_string()]).unwrap();
        }
        crate::factory::store::add(&conn, "p1", "local").unwrap();
        conn
    }

    #[test]
    fn the_default_team_is_the_prototypes_on_real_accounts() {
        let c = conn_with_accounts(&[("work", "claude"), ("personal", "claude"), ("cx", "codex")]);
        let team = load(&c, "p1").unwrap();
        assert_eq!(team.len(), 13);
        let by = |name: &str| team.iter().find(|a| a.name == name).unwrap();
        assert_eq!((by("Spec writer").account_id.as_deref(), by("Spec writer").model.as_str()), (Some("work"), "opus"));
        assert_eq!(by("Crítico R0").account_id.as_deref(), Some("personal"));
        assert_eq!((by("Aprobador").account_id.as_deref(), by("Aprobador").model.as_str()), (Some("cx"), "gpt-5-codex"), "a model the account offers");
        assert_eq!(by("Builder").escalate_model.as_deref(), Some("opus"));
        assert!(!by("Tester E2E").active);
        let check = check(&c, &team).unwrap();
        assert!(check.problems.is_empty() && check.warnings.is_empty(), "{check:?}");
        assert_eq!(load(&c, "p1").unwrap(), team, "seeded once, then read back");
    }

    #[test]
    fn the_brain_says_how_the_project_is_tested() {
        let c = conn_with_accounts(&[("work", "claude"), ("cx", "codex")]);
        let md = "## Producto\n- PRD-01 · DEBE · x\n## Arquitectura\n- ARQ-01 · DEBE · x\n## Tecnologías\n- TEC-01 · DEBE · x\n## Código\n- COD-01 · DEBE · x\n\
## Calidad\n- CAL-01 · DEBE · Cobertura.  check: `pnpm test` (`vitest run --coverage`)\n\
- CAL-02 · DEBE · e2e.  check: `pnpm e2e --list | grep -q firefox && pnpm e2e`\n- CAL-04 · DEBE · axe.  check: `pnpm e2e`\n\
- CAL-05 · DEBE · lh.  check: `pnpm lhci autorun`\n- CAL-07 · DEBE · fix.  check: `git show --name-only HEAD | grep -qE '\\.(test|spec)\\.tsx?$'`\n\
## Git y entrega\n- GIT-01 · DEBE · x\n## Riesgo\n- RSG-01 · DEBE · x\n";
        let root = std::env::temp_dir().join(format!("hangar-team-{}", uuid::Uuid::new_v4().simple()));
        let brain = super::super::brain::seal(&c, &root, "p1", "POK", md, "import").unwrap();
        assert_eq!(brain_test_commands(&brain), (Some("pnpm test".into()), None, Some("pnpm e2e".into())));
        let team = load(&c, "p1").unwrap();
        let s = suggestions(&team, &brain);
        let by = |n: &str| s.iter().find(|x| x.name == n).cloned();
        assert_eq!(by("Tester unitario").unwrap().suggested.as_deref(), Some("pnpm test"));
        assert!(!by("Tester de integración").unwrap().active, "no integration tests in the brain: off");
        assert_eq!((by("Tester E2E").unwrap().suggested.as_deref(), by("Tester E2E").unwrap().active), (Some("pnpm e2e"), true));
        assert_eq!(apply_suggestions(&c, "p1", &brain).unwrap(), 3);
        assert!(suggestions(&load(&c, "p1").unwrap(), &brain).is_empty(), "applied: nothing left to suggest");
        let _ = std::fs::remove_dir_all(&root);
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
    fn a_team_on_one_family_runs_without_an_approver() {
        let c = conn_with_accounts(&[("work", "claude")]);
        let mut team = load(&c, "p1").unwrap();
        let approver = team.iter_mut().find(|a| a.role == "approver").unwrap();
        assert!(!approver.required, "it can be switched off");
        approver.active = false;
        save(&c, "p1", &team).unwrap();
        let check = check(&c, &load(&c, "p1").unwrap()).unwrap();
        assert!(check.problems.is_empty() && check.warnings.is_empty(), "no approver, no same-family warning: {check:?}");
        // A team saved when the approver was obligatory reads as optional.
        c.execute("UPDATE factory_agents SET required = 1 WHERE role = 'approver'", []).unwrap();
        assert!(!load(&c, "p1").unwrap().iter().find(|a| a.role == "approver").unwrap().required);
    }

    #[test]
    fn codex_offers_the_models_its_login_lists() {
        let dir = std::env::temp_dir().join(format!("hangar-codex-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let d = dir.to_string_lossy().to_string();
        std::fs::write(dir.join("config.toml"), "model = \"gpt-5.6-terra\"\n[projects.\"/x\"]\nmodel = \"no\"\n").unwrap();
        assert_eq!(models_for("codex", None, Some(&d)), vec!["gpt-5.6-terra"], "no catalog yet: its config's model");
        std::fs::write(
            dir.join("models_cache.json"),
            r#"{"models":[{"slug":"gpt-reserve","visibility":"hide"},{"slug":"gpt-6-luna","visibility":"list"},{"slug":"gpt-5.6-luna","visibility":"list"}]}"#,
        )
        .unwrap();
        assert_eq!(models_for("codex", None, Some(&d)), vec!["gpt-6-luna", "gpt-5.6-luna"]);

        // An approver left on `gpt-5`, which a ChatGPT login can't use, blocks.
        let c = conn_with_accounts(&[("work", "claude"), ("cx", "codex")]);
        c.execute("UPDATE accounts SET config_dir_path = ?1 WHERE id = 'cx'", params![d]).unwrap();
        let mut team = load(&c, "p1").unwrap();
        team.iter_mut().find(|a| a.role == "approver").unwrap().model = "gpt-5".into();
        let check = check(&c, &team).unwrap();
        assert!(check.problems.iter().any(|p| p.contains("no ofrece el modelo gpt-5")), "{check:?}");
        let _ = std::fs::remove_dir_all(&dir);
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
