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

    // git again, before taking the database lock: an empty repo gets order 0.
    let empty = (repo_ok || fix) && is_empty_repo(dir);

    // Everything that reads or writes the database, in one short stretch with
    // the lock held — the rest of Hangar waits on it. Nothing slow in here: no
    // processes, no keychain, no network.
    let mut queue_checks = Vec::new();
    let (sealed, planned, account_rows) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let sealed = match project.brain_version {
            None => {
                out.push(check("brain", "Brain", "blocked", "sin sellar: la línea decide con él", Some("brain")));
                None
            }
            Some(v) => {
                // The brain, with its index rebuilt by this parser.
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
        // The team as it will be once the brain's tester commands are applied:
        // what the tools check judges, so a command about to be replaced isn't
        // reported missing on top of the suggestion that replaces it.
        let mut planned = members.clone();
        if let Some(s) = &sealed {
            let changes = team::suggestions(&members, s);
            if !changes.is_empty() {
                let words: Vec<String> = changes
                    .iter()
                    .map(|c| match &c.suggested {
                        Some(cmd) if c.active => format!("{} → `{cmd}`", c.short()),
                        _ => format!("{} → apagado", c.short()),
                    })
                    .collect();
                let old: Vec<String> = changes.iter().filter_map(|c| c.current.clone()).filter_map(|c| env::first_program(&c)).collect::<std::collections::BTreeSet<_>>().into_iter().collect();
                let lead = if old.is_empty() { "los testers no usan lo que dice el brain".to_string() } else { format!("los testers usan comandos de otro stack ({})", old.join(", ")) };
                out.push(check("testers", "Testers", "blocked", format!("{lead}; el brain dice: {}. Pulsa Aplicar.", words.join(" · ")), Some("apply_testers")));
                for c in &changes {
                    if let Some(a) = planned.iter_mut().find(|a| a.id == c.agent_id) {
                        a.active = c.active;
                        if let Some(cmd) = &c.suggested {
                            a.command = Some(cmd.clone());
                        }
                    }
                }
            }
        }
        let ids: Vec<String> = members.iter().filter(|a| a.active || a.required).filter_map(|a| a.account_id.clone()).collect();
        let account_rows = super::agent::accounts_for(&conn, &ids);

        // The queue: orders stopped by the project, and order 0 for an empty one.
        if fix {
            let back = orders::reset_never_started(&conn, project_id)?;
            if !back.is_empty() {
                queue_checks.push(check("orders", "Órdenes", "fixed", format!("{} vuelve a la cola: falló antes de empezar, por el proyecto", back.join(", ")), None));
            }
        }
        if empty && !orders::list(&conn, project_id)?.iter().any(|o| o.origin == "bootstrap") {
            let prefix = orders::key_prefix(&project.project_name);
            if fix {
                orders::add_bootstrap(&conn, project_id, &prefix)?;
                queue_checks.push(check("bootstrap", "Orden 0", "fixed", format!("el repo está vacío: {prefix}-0 «Preparar el proyecto» va primero y deja ejecutable cada comando de la puerta"), None));
            } else {
                queue_checks.push(check("bootstrap", "Orden 0", "todo", format!("el repo está vacío: al preparar se añade {prefix}-0 «Preparar el proyecto»"), None));
            }
        }
        let todo = orders::list(&conn, project_id)?.iter().filter(|o| o.status == "todo").count();
        if todo == 0 && line::active_run(&conn, project_id)?.is_none() {
            queue_checks.push(check(
                "tasks",
                "Tareas",
                "blocked",
                if project.provider_kind == "none" { "no hay tareas: pon un proveedor o créalas desde un TRD" } else { "no hay tareas en la cola: tráelas del proveedor o créalas desde un TRD" },
                Some("tasks"),
            ));
        }
        (sealed, planned, account_rows)
    };

    // From here on, without the lock: the filesystem, the login shell, the
    // keychain.

    // The machine, as the brain's «Entorno» says it must be: a red here would
    // otherwise be found by a builder that can't fix it.
    if let Some(s) = &sealed {
        let env_rules: Vec<brain::Rule> = s.rules.iter().filter(|r| r.section == "ent").cloned().collect();
        if !env_rules.is_empty() {
            let failing = super::lint::lint_env(&env_rules, project_id, dir);
            if failing.is_empty() {
                out.push(check("entorno", "Entorno", "ok", format!("la máquina cumple los {} checks de Entorno", env_rules.iter().filter(|r| r.check.is_some()).count()), None));
            }
            for f in failing {
                out.push(check("entorno", "Entorno", "blocked", format!("{}: {}", f.rule_id, f.message.lines().next().unwrap_or("")), None));
            }
        }
    }

    // Every program the gate will call has to exist.
    if let Some(s) = &sealed {
        let mut missing: Vec<String> = line::gate_commands(&planned, s, &Default::default(), true).iter().filter_map(|(_, c)| env::first_program(c)).filter(|p| !p.contains('/') && !env::has_executable(p)).collect();
        missing.sort();
        missing.dedup();
        if !missing.is_empty() {
            out.push(check("tools", "Herramientas", "blocked", format!("falta {} en esta máquina: instálalo o cambia el comando que lo usa", missing.join(", ")), Some("team")));
        } else {
            out.push(check("tools", "Herramientas", "ok", "todos los programas de la puerta están instalados", None));
        }
    }

    // The CLIs the team's accounts run.
    match account_rows {
        Err(e) => out.push(check("cli", "CLIs", "blocked", e, Some("team"))),
        Ok(rows) => {
            let mut bins: Vec<&str> = rows
                .iter()
                .map(|(_, acc)| if acc.provider == "codex" { "codex" } else { "claude" })
                .filter(|bin| {
                    let path = if *bin == "claude" { crate::providers::claude::binary() } else { crate::providers::resolve_binary(bin) };
                    !Path::new(&path).is_file() && !env::has_executable(bin)
                })
                .collect();
            bins.sort();
            bins.dedup();
            if let Err(e) = super::agent::CliRunner::from_accounts(rows, None) {
                out.push(check("cli", "CLIs", "blocked", e, Some("team")));
            } else if !bins.is_empty() {
                out.push(check("cli", "CLIs", "blocked", format!("no se encuentra {}: instálalo para las cuentas del equipo", bins.join(", ")), Some("team")));
            }
        }
    }

    out.extend(queue_checks);
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
    fn a_tester_command_the_brain_replaces_is_not_reported_missing_too() {
        let w = line::tests::world("autonomous", "local", "pytest-not-installed-xyz -q");
        {
            let conn = w.db.0.lock().unwrap();
            // The brain says how this project is tested, with a runner that
            // exists here. Looked at without fixing: fixing re-indexes the
            // brain from its file, which has no such check.
            conn.execute("UPDATE factory_brain_rules SET check_cmd = 'python3 -c \"print(1)\" # test' WHERE project_id = 'p1' AND rule_id = 'CAL-01'", []).unwrap();
        }
        let checks = prepare(&w.db, "p1", &setup::tests_identity(), false).unwrap();
        let testers = checks.iter().find(|c| c.id == "testers").expect("the brain's suggestion");
        assert!(testers.detail.contains("pytest-not-installed-xyz") && testers.detail.contains("Pulsa Aplicar"), "{}", testers.detail);
        assert_eq!(checks.iter().find(|c| c.id == "tools").unwrap().status, "ok", "{checks:?}");
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

    #[test]
    fn a_machine_short_of_the_brains_entorno_blocks_play() {
        use rusqlite::params;
        let w = line::tests::world("autonomous", "local", "true");
        {
            let conn = w.db.0.lock().unwrap();
            let v: i64 = conn.query_row("SELECT brain_version FROM factory_projects WHERE project_id = 'p1'", [], |r| r.get(0)).unwrap();
            conn.execute(
                "INSERT INTO factory_brain_rules (project_id, version, rule_id, section, level, text, check_cmd, position) VALUES ('p1', ?1, 'ENT-01', 'ent', 'must', 'Firefox arranca', ?2, 999)",
                params![v, "echo 'browserType.launch: Failed to launch the browser process' >&2; exit 1"],
            )
            .unwrap();
        }
        // `fix` would rebuild the index from the brain's file, without this row.
        let checks = prepare(&w.db, "p1", &setup::tests_identity(), false).unwrap();
        let env = checks.iter().find(|c| c.id == "entorno").unwrap();
        assert_eq!(env.status, "blocked");
        assert!(env.detail.starts_with("ENT-01"));
        assert!(blocking(&checks).is_err(), "▶ doesn't start on a machine that can't run the suite");
    }
}
