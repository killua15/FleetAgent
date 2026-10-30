//! Getting a project ready for the line, so ▶ doesn't stumble on something
//! that could have been seen — or fixed — before. What can be fixed without
//! asking is fixed (git, `dev`, the brain's index, order 0, orders that never
//! got to start); what needs you says so, with where to fix it.

use std::path::Path;

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};

use super::{brain, env, line, orders, setup, team};
use crate::db::DbConnection;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub id: &'static str,
    pub label: &'static str,
    /// `ok`, `fixed` (just now), `todo` (fixed on «Preparar»), `warn` or
    /// `blocked` (▶ won't start until you fix it).
    pub status: &'static str,
    pub detail: String,
    /// Where to fix it: `apply_testers`, `team`, `brain`, `git_identity`, `tasks`.
    pub action: Option<&'static str>,
}

fn check(id: &'static str, label: &'static str, status: &'static str, detail: impl Into<String>, action: Option<&'static str>) -> Check {
    Check { id, label, status, detail: detail.into(), action }
}

/// Files a repo can have and still be «empty» for the line: the brain and
/// its kin, and editor or tool settings.
fn is_scaffolding(path: &str) -> bool {
    const FILES: [&str; 7] = ["BRAIN.md", "CLAUDE.md", "AGENTS.md", "README.md", ".gitignore", "LICENSE", "LICENSE.md"];
    FILES.contains(&path) || path.starts_with(".claude/") || path.starts_with(".hangar/") || path.starts_with(".vscode/") || path.starts_with(".idea/")
}

pub fn is_empty_repo(dir: &Path) -> bool {
    match crate::commands::pull_requests::git_capture(dir, &["ls-files"]) {
        Ok(files) => files.lines().all(|f| f.trim().is_empty() || is_scaffolding(f.trim())),
        Err(_) => true,
    }
}

fn git(dir: &Path, args: &[&str], env: &[(String, String)]) -> Result<String, String> {
    crate::commands::pull_requests::git_capture_with_env(dir, args, env)
}

/// Every check, fixing what can be fixed when `fix`.
pub fn prepare(db: &DbConnection, project_id: &str, git_env: &[(String, String)], fix: bool) -> Result<Vec<Check>, String> {
    let project = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        super::store::get(&conn, project_id)?
    };
    let dir = Path::new(&project.project_path);
    let mut out = Vec::new();

    // git: a repo with commits, `main` and `dev`, `.hangar/` kept out.
    let status = setup::repo_status(dir);
    if !status.exists {
        out.push(check("repo", "Repositorio", "blocked", format!("{} no existe", project.project_path), None));
        return Ok(out);
    }
    let mut repo_ok = status.is_repo && status.has_commits;
    if !repo_ok {
        if !fix {
            out.push(check("repo", "Repositorio", "todo", "no es un repo git con commits · se inicializa al preparar (main con un primer commit + dev)", None));
        } else {
            let made = if status.is_repo {
                git(dir, &["add", "-A"], git_env).and_then(|_| git(dir, &["commit", "--allow-empty", "-m", "Initial commit"], git_env)).map(|_| ())
            } else {
                setup::init_repo(dir, git_env)
            };
            match made {
                Ok(()) => {
                    repo_ok = true;
                    out.push(check("repo", "Repositorio", "fixed", "git inicializado: main con un primer commit de lo que había, y dev", None));
                }
                Err(e) => out.push(check("repo", "Repositorio", "blocked", format!("no se pudo inicializar git: {e}"), Some("git_identity"))),
            }
        }
    } else {
        out.push(check("repo", "Repositorio", "ok", format!("rama base {}", status.base_branch.clone().unwrap_or_else(|| "main".into())), None));
    }
    if repo_ok {
        let status = setup::repo_status(dir);
        if status.has_main && !status.has_dev {
            if fix {
                match git(dir, &["branch", "dev", "main"], &[]) {
                    Ok(_) => out.push(check("dev", "Rama dev", "fixed", "creada desde main: las órdenes salen de dev", None)),
                    Err(e) => out.push(check("dev", "Rama dev", "warn", format!("no se pudo crear: {e} · las órdenes saldrán de main"), None)),
                }
            } else {
                out.push(check("dev", "Rama dev", "todo", "no existe · se crea desde main al preparar", None));
            }
        }
        if fix {
            crate::task_sources::exclude_hangar_dir_locally(dir);
        }
    }

    // Who signs the commits.
    let has_identity = git_env.iter().any(|(k, _)| k == "GIT_AUTHOR_EMAIL") || git(dir, &["config", "user.email"], &[]).is_ok_and(|e| !e.trim().is_empty());
    if !has_identity {
        out.push(check("identity", "Identidad git", "blocked", "sin nombre ni email para los commits: configúralos en Settings → Git identity", Some("git_identity")));
    }

    // The brain, with its index rebuilt by this parser.
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let sealed = match project.brain_version {
        None => {
            out.push(check("brain", "Brain", "blocked", "sin sellar: la línea decide con él", Some("brain")));
            None
        }
        Some(v) => {
            if fix {
                brain::reindex(&conn, project_id, v)?;
            }
            let s = brain::load(&conn, project_id, Some(v))?;
            if let Some(s) = &s {
                out.push(check("brain", "Brain", "ok", format!("v{v} · {} reglas · {} con check:", s.rules.len(), s.rules.iter().filter(|r| r.check.is_some()).count()), None));
            }
            s
        }
    };

    // The team, and how the brain says the project is tested.
    let members = team::load(&conn, project_id)?;
    let verdict = team::check(&conn, &members)?;
    for p in &verdict.problems {
        out.push(check("team", "Equipo", "blocked", p.clone(), Some("team")));
    }
    for w in &verdict.warnings {
        out.push(check("team", "Equipo", "warn", w.clone(), Some("team")));
    }
    if let Some(s) = &sealed {
        let changes = team::suggestions(&members, s);
        if !changes.is_empty() {
            let words: Vec<String> = changes
                .iter()
                .map(|c| match &c.suggested {
                    Some(cmd) if c.active => format!("{} → `{cmd}`", c.name),
                    _ => format!("{} → apagado", c.name),
                })
                .collect();
            out.push(check("testers", "Testers", "blocked", format!("el brain dice cómo se prueba este proyecto: {}", words.join(" · ")), Some("apply_testers")));
        }
    }

    // Every program the gate will call has to exist.
    if let Some(s) = &sealed {
        let mut missing: Vec<String> = line::gate_commands(&members, s, &Default::default(), true).iter().filter_map(|(_, c)| env::first_program(c)).filter(|p| !p.contains('/') && !env::has_executable(p)).collect();
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            out.push(check("tools", "Herramientas", "blocked", format!("falta {} en esta máquina: instálalo o cambia el comando que lo usa", missing.join(", ")), Some("team")));
        } else {
            out.push(check("tools", "Herramientas", "ok", "todos los programas de la puerta están instalados", None));
        }
    }

    // The CLIs the team's accounts run.
    let accounts: Vec<String> = members.iter().filter(|a| a.active || a.required).filter_map(|a| a.account_id.clone()).collect();
    if let Err(e) = super::agent::CliRunner::for_accounts(&conn, &accounts, None) {
        out.push(check("cli", "CLIs", "blocked", e, Some("team")));
    } else {
        let mut bins = Vec::new();
        for id in &accounts {
            if let Ok(acc) = crate::terminals::load_account(&conn, id) {
                let bin = if acc.provider == "codex" { "codex" } else { "claude" };
                let path = if bin == "claude" { crate::providers::claude::binary() } else { crate::providers::resolve_binary(bin) };
                if !Path::new(&path).is_file() && !env::has_executable(bin) {
                    bins.push(bin);
                }
            }
        }
        bins.sort();
        bins.dedup();
        if !bins.is_empty() {
            out.push(check("cli", "CLIs", "blocked", format!("no se encuentra {}: instálalo para las cuentas del equipo", bins.join(", ")), Some("team")));
        }
    }

    // The queue: orders stopped by the project, and order 0 for an empty one.
    if fix {
        let back = orders::reset_never_started(&conn, project_id)?;
        if !back.is_empty() {
            out.push(check("orders", "Órdenes", "fixed", format!("{} vuelve a la cola: falló antes de empezar, por el proyecto", back.join(", ")), None));
        }
    }
    if repo_ok || fix {
        let empty = is_empty_repo(dir);
        let has_boot = orders::list(&conn, project_id)?.iter().any(|o| o.origin == "bootstrap");
        if empty && !has_boot {
            let prefix = orders::key_prefix(&project.project_name);
            if fix {
                orders::add_bootstrap(&conn, project_id, &prefix)?;
                out.push(check("bootstrap", "Orden 0", "fixed", format!("el repo está vacío: {prefix}-0 «Preparar el proyecto» va primero y deja ejecutable cada comando de la puerta"), None));
            } else {
                out.push(check("bootstrap", "Orden 0", "todo", format!("el repo está vacío: al preparar se añade {prefix}-0 «Preparar el proyecto»"), None));
            }
        }
    }
    let todo = orders::list(&conn, project_id)?.iter().filter(|o| o.status == "todo").count();
    if todo == 0 && line::active_run(&conn, project_id)?.is_none() {
        out.push(check(
            "tasks",
            "Tareas",
            "blocked",
            if project.provider_kind == "none" { "no hay tareas: pon un proveedor o créalas desde un TRD" } else { "no hay tareas en la cola: tráelas del proveedor o créalas desde un TRD" },
            Some("tasks"),
        ));
    }
    Ok(out)
}

/// The blocking problems, in one sentence each, or `Ok`.
pub fn blocking(checks: &[Check]) -> Result<(), String> {
    let blocked: Vec<String> = checks.iter().filter(|c| c.status == "blocked").map(|c| format!("{}: {}", c.label, c.detail)).collect();
    if blocked.is_empty() {
        Ok(())
    } else {
        Err(blocked.join("\n"))
    }
}

#[tauri::command]
pub async fn factory_prepare<R: Runtime>(app: AppHandle<R>, project_id: String, fix: bool) -> Result<Vec<Check>, String> {
    crate::blocking::run_blocking(move || {
        let env = crate::git_identity::git_env_for_project(&app, &project_id);
        let db = app.state::<DbConnection>();
        let checks = prepare(&db, &project_id, &env, fix)?;
        let _ = tauri::Emitter::emit(&app, "factory-updated", &project_id);
        Ok(checks)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_without_git_is_initialised_and_an_empty_repo_gets_order_zero() {
        let w = line::tests::world("autonomous", "local", "true");
        std::fs::remove_dir_all(w.repo.join(".git")).unwrap();
        std::fs::write(w.repo.join("BRAIN.md"), "# brain\n").unwrap();
        let env = setup::tests_identity();

        let seen = prepare(&w.db, "p1", &env, false).unwrap();
        assert_eq!(seen.iter().find(|c| c.id == "repo").unwrap().status, "todo", "looking doesn't touch anything");
        assert!(!w.repo.join(".git").exists());

        let done = prepare(&w.db, "p1", &env, true).unwrap();
        assert_eq!(done.iter().find(|c| c.id == "repo").unwrap().status, "fixed");
        let s = setup::repo_status(&w.repo);
        assert!(s.is_repo && s.has_commits && s.has_main && s.has_dev);
        assert_eq!(done.iter().find(|c| c.id == "bootstrap").unwrap().status, "fixed", "only BRAIN.md: empty");
        let conn = w.db.0.lock().unwrap();
        let boot = orders::next(&conn, "p1").unwrap().unwrap();
        assert_eq!(boot.key, "VET-0");
        drop(conn);
        // Run again: nothing left to fix.
        let again = prepare(&w.db, "p1", &env, true).unwrap();
        assert!(again.iter().all(|c| c.status != "fixed"), "{again:?}");
    }

    #[test]
    fn a_tool_the_gate_needs_and_lacks_blocks_play() {
        let w = line::tests::world("autonomous", "local", "definitely-not-installed-xyz --run");
        let checks = prepare(&w.db, "p1", &setup::tests_identity(), true).unwrap();
        let tools = checks.iter().find(|c| c.id == "tools").unwrap();
        assert_eq!(tools.status, "blocked");
        assert!(tools.detail.contains("definitely-not-installed-xyz"));
        assert!(blocking(&checks).is_err());
    }
}
