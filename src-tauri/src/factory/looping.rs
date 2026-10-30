//! The loop: ▶ Play takes the queue one order at a time, in its order, until
//! it's empty or you stop it. A usage limit makes it wait and go on by itself
//! when the window resets; a lack of credits, a ✋ or three failures in a row
//! make it stop and say so. It survives Hangar restarting.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use super::agent::CliRunner;
use super::line::{self, Ctx, Run};
use super::orders;
use crate::db::DbConnection;

/// Failed orders in a row before the loop stops itself.
pub const BREAKER: usize = 3;

const TICK: Duration = Duration::from_secs(2);
/// How often delivered MRs are asked whether they were merged.
const MERGE_POLL: Duration = Duration::from_secs(120);

/// One worker per project, and a word on what it's doing for the UI.
struct Worker {
    wake: AtomicBool,
}

static WORKERS: LazyLock<Mutex<HashMap<String, Arc<Worker>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Project → what the worker is waiting for (`quota`, `merges`), if a wait,
/// and the words for it.
static NOTES: LazyLock<Mutex<HashMap<String, (Option<&'static str>, String)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Projects whose next order starts even with the quota check against it.
static START_ANYWAY: LazyLock<Mutex<std::collections::HashSet<String>>> = LazyLock::new(|| Mutex::new(Default::default()));

fn set_note(project_id: &str, note: Option<&str>) {
    set_wait(project_id, None, note);
}

fn set_wait(project_id: &str, kind: Option<&'static str>, note: Option<&str>) {
    if let Ok(mut n) = NOTES.lock() {
        match note {
            Some(text) => n.insert(project_id.to_string(), (kind, text.to_string())),
            None => n.remove(project_id),
        };
    }
}

fn set_state(conn: &Connection, project_id: &str, state: &str) -> Result<(), String> {
    conn.execute("UPDATE factory_projects SET loop_state = ?2, updated_at = ?3 WHERE project_id = ?1", params![project_id, state, Utc::now().to_rfc3339()]).map_err(|e| e.to_string())?;
    Ok(())
}

fn loop_state(conn: &Connection, project_id: &str) -> String {
    conn.query_row("SELECT loop_state FROM factory_projects WHERE project_id = ?1", params![project_id], |r| r.get(0)).unwrap_or_else(|_| "stopped".into())
}

/// Usage at which the Factory doesn't start an order on an account. Higher
/// than the run queue's 70 %: the line notices the real limit itself and
/// waits for the reset without counting it a failure, so holding back early
/// only idles an account that still has room.
pub const MAX_START_PCT: f64 = 95.0;

/// Whether the account can start an order now, per Hangar's quota samples;
/// `Err` says why not. An account never sampled has room.
pub fn room(conn: &Connection, account_id: &str, root: Option<&std::path::Path>) -> Result<(), String> {
    let Ok(quotas) = crate::quota::account_quotas(conn) else { return Ok(()) };
    let Some(q) = quotas.iter().find(|q| q.account_id == account_id) else { return Ok(()) };
    let reset = root
        .and_then(|r| crate::auto_continue::latest_rate_limits(r, account_id))
        .and_then(|l| crate::auto_continue::resume_at(&l, 0))
        .map(|at| format!(" · se renueva a las {}", at.with_timezone(&chrono::Local).format("%H:%M")))
        .unwrap_or_default();
    if q.status != "healthy" {
        return Err(format!("«{}» está {}{reset}", q.account_name, if q.status == "exhausted" { "agotada" } else { "enfriándose" }));
    }
    for (window, label) in [(&q.session, "de la sesión"), (&q.weekly, "de la semana")] {
        if let Some(f) = window {
            if f.pct >= MAX_START_PCT {
                return Err(format!("«{}» va por el {:.0} % {label}{reset}", q.account_name, f.pct));
            }
        }
    }
    Ok(())
}

/// What ▶ would do now, decided from the queue alone — the part that can be
/// tested without a worker.
#[derive(Debug, PartialEq)]
pub enum Next {
    /// Go on with this run (waiting, stopped or ✋ resolved).
    Resume(String),
    /// Start this order.
    Start(String),
    /// Orders remain, but they wait for MRs to be merged.
    WaitMerges,
    /// Orders remain, but they wait on orders that failed or were stopped:
    /// nothing moves until one of those is retried.
    Blocked(Vec<String>),
    /// Nothing to do.
    Empty,
}

pub fn decide(conn: &Connection, project_id: &str) -> Result<Next, String> {
    if let Some(run) = line::active_run(conn, project_id)? {
        return Ok(Next::Resume(run.id));
    }
    if let Some(order) = orders::next(conn, project_id)? {
        return Ok(Next::Start(order.id));
    }
    let all = orders::list(conn, project_id)?;
    if all.iter().any(|o| o.status == "delivered") {
        return Ok(Next::WaitMerges);
    }
    if !all.iter().any(|o| o.status == "todo") {
        return Ok(Next::Empty);
    }
    let waited_on: std::collections::HashSet<&str> = all.iter().filter(|o| o.status == "todo").flat_map(|o| o.deps.iter().map(String::as_str)).collect();
    let by = all.iter().filter(|o| matches!(o.status.as_str(), "failed" | "stopped") && waited_on.contains(o.key.as_str())).map(|o| o.key.clone()).collect();
    Ok(Next::Blocked(by))
}

/// Consecutive failed orders, newest first, up to the last that wasn't.
pub fn failures_in_a_row(conn: &Connection, project_id: &str) -> Result<usize, String> {
    Ok(line::runs(conn, project_id, BREAKER as i64)?.iter().take_while(|r| r.status == "failed").count())
}

fn emit<R: Runtime>(app: &AppHandle<R>, project_id: &str) {
    let _ = app.emit("factory-updated", project_id);
}

fn notify<R: Runtime>(app: &AppHandle<R>, title: &str, body: &str) {
    crate::notifications::record(app, crate::notifications::kind::AGENT, |s| s.notify_session_waiting, title, body, crate::notifications::Target::screen("factory"));
}

/// Asks the forge about every open MR of the project; a merged one unblocks
/// what depends on it.
fn poll_merges<R: Runtime>(app: &AppHandle<R>, project_id: &str) {
    let db = app.state::<DbConnection>();
    let open: Vec<Run> = match db.0.lock() {
        Ok(conn) => line::runs(&conn, project_id, 100).unwrap_or_default().into_iter().filter(|r| r.status == "delivered" && r.mr_number.is_some()).collect(),
        Err(_) => return,
    };
    for mut run in open {
        let Ok(repo) = crate::forges::detect_repo(std::path::Path::new(&run.worktree)) else { continue };
        let Ok(facts) = crate::forges::fetch_single_pr_facts(&repo, run.mr_number.unwrap_or_default()) else { continue };
        if facts.state == "merged" {
            run.status = "merged".into();
            if let Ok(conn) = db.0.lock() {
                let _ = line::save_run(&conn, &run);
                if let Ok(order) = orders::get(&conn, &run.order_id) {
                    drop(conn);
                    notify(app, &format!("{} fusionada", order.key), "La MR se fusionó: lo que dependía de ella ya puede empezar.");
                }
            }
            emit(app, project_id);
        }
    }
}

/// The worker: runs until the queue is empty, ⏹, a pause finishing its
/// order, or the breaker.
fn work<R: Runtime>(app: AppHandle<R>, project_id: String, worker: Arc<Worker>) {
    let db = app.state::<DbConnection>();
    let mut last_merge_poll: Option<std::time::Instant> = None;
    loop {
        let state = match db.0.lock() {
            Ok(conn) => loop_state(&conn, &project_id),
            Err(_) => break,
        };
        if state == "stopped" {
            break;
        }
        if last_merge_poll.is_none_or(|t| t.elapsed() >= MERGE_POLL) {
            poll_merges(&app, &project_id);
            last_merge_poll = Some(std::time::Instant::now());
        }
        let next = match db.0.lock() {
            Ok(conn) => decide(&conn, &project_id),
            Err(_) => break,
        };
        let next = match next {
            Ok(n) => n,
            Err(e) => {
                set_note(&project_id, Some(&e));
                break;
            }
        };
        match next {
            Next::Resume(run_id) => {
                let run = match db.0.lock() {
                    Ok(conn) => line::load_run(&conn, &run_id),
                    Err(_) => break,
                };
                let Ok(run) = run else { break };
                let due = match run.status.as_str() {
                    "running" => true,
                    "waiting_limit" => run.resume_at.as_deref().and_then(|t| DateTime::parse_from_rfc3339(t).ok()).is_none_or(|t| t.with_timezone(&Utc) <= Utc::now()),
                    // A ⏹'d order goes on when ▶ is pressed again.
                    "stopped" => true,
                    // ✋ waits for a person; Resolve wakes this up.
                    _ => false,
                };
                if !due {
                    set_note(&project_id, Some(if run.status == "waiting_limit" { "en espera por límite de uso" } else { "esperando a una persona" }));
                    sleep_or_wake(&worker, &app, &project_id);
                    continue;
                }
                set_note(&project_id, None);
                drive(&app, &project_id, &run_id);
                // After an order: pause means stop here; the breaker too.
                let conn = match db.0.lock() {
                    Ok(c) => c,
                    Err(_) => break,
                };
                let finished = line::load_run(&conn, &run_id).map(|r| matches!(r.status.as_str(), "delivered" | "merged" | "failed")).unwrap_or(true);
                if finished && loop_state(&conn, &project_id) == "paused" {
                    let _ = set_state(&conn, &project_id, "stopped");
                    set_note(&project_id, Some("en pausa: la orden en curso terminó"));
                    break;
                }
                if failures_in_a_row(&conn, &project_id).unwrap_or(0) >= BREAKER {
                    let _ = set_state(&conn, &project_id, "stopped");
                    drop(conn);
                    set_note(&project_id, Some("parada: 3 órdenes fallidas seguidas"));
                    notify(&app, "La Factory se paró", "Tres órdenes seguidas fallaron. Revísalas en Entregas antes de seguir.");
                    emit(&app, &project_id);
                    break;
                }
            }
            Next::Start(order_id) => {
                let (spec_account, paused) = match db.0.lock() {
                    Ok(conn) => (
                        super::team::load(&conn, &project_id).ok().and_then(|t| t.into_iter().find(|a| a.role == "spec").and_then(|a| a.account_id)),
                        loop_state(&conn, &project_id) == "paused",
                    ),
                    Err(_) => break,
                };
                if paused {
                    if let Ok(conn) = db.0.lock() {
                        let _ = set_state(&conn, &project_id, "stopped");
                    }
                    break;
                }
                // Look at the quota before starting, not after the first
                // refusal — unless you said to start anyway.
                let anyway = START_ANYWAY.lock().map(|mut s| s.remove(&project_id)).unwrap_or(false);
                if let (Some(acc), false) = (&spec_account, anyway) {
                    let root = crate::app_data::root(&app).ok();
                    let verdict = db.0.lock().map(|c| room(&c, acc, root.as_deref())).unwrap_or(Ok(()));
                    if let Err(why) = verdict {
                        set_wait(&project_id, Some("quota"), Some(&format!("esperando cuota: {why}")));
                        sleep_long(&worker, &app, &project_id, Duration::from_secs(120));
                        continue;
                    }
                }
                if let Some(Err(why)) = db.0.lock().ok().map(|c| line::ready(&c, &project_id)) {
                    halt(&app, &project_id, &why);
                    break;
                }
                set_note(&project_id, None);
                let ctx_parts = context(&app, &project_id);
                let started = {
                    let notify_fn = |t: &str, b: &str| notify(&app, t, b);
                    let changed_fn = || emit(&app, &project_id);
                    let runner = match ctx_parts.runner(&db, &project_id) {
                        Ok(r) => r,
                        Err(e) => {
                            halt(&app, &project_id, &e);
                            break;
                        }
                    };
                    let ctx = Ctx { db: &db, runner: &runner, git_env: ctx_parts.git_env.clone(), root: ctx_parts.root.clone(), notify: &notify_fn, changed: &changed_fn };
                    line::start(&ctx, &project_id, &order_id)
                };
                match started {
                    Ok(_) => emit(&app, &project_id),
                    Err(e) => {
                        // The project was ready, so this is the order's own
                        // failure: it's marked, and the queue moves on.
                        if let Ok(conn) = db.0.lock() {
                            let _ = orders::set_status(&conn, &order_id, "failed");
                        }
                        set_note(&project_id, Some(&e));
                        notify(&app, "Una orden no pudo empezar", &e);
                        emit(&app, &project_id);
                    }
                }
            }
            Next::WaitMerges => {
                set_wait(&project_id, Some("merges"), Some("esperando a que se fusionen las MR de las que dependen las demás"));
                sleep_long(&worker, &app, &project_id, MERGE_POLL);
                last_merge_poll = None;
            }
            Next::Blocked(by) => {
                let keys = if by.is_empty() { "una orden anterior".to_string() } else { by.join(", ") };
                let why = format!("{keys} {} y las demás dependen de {}: reintenta {keys} en la Cola", if by.len() > 1 { "fallaron" } else { "falló" }, if by.len() > 1 { "ellas" } else { "ella" });
                if let Ok(conn) = db.0.lock() {
                    let _ = set_state(&conn, &project_id, "stopped");
                }
                set_wait(&project_id, Some("blocked"), Some(&why));
                notify(&app, "La Factory se paró", &why);
                emit(&app, &project_id);
                break;
            }
            Next::Empty => {
                // Every order passed its own checks; before calling it done,
                // the whole brain runs once on the base.
                set_wait(&project_id, Some("verify"), Some("verificación completa: corriendo todos los check: del brain en la base…"));
                emit(&app, &project_id);
                let verdict = verify_all(&app, &project_id);
                match verdict {
                    Ok(Verification::NewOrders(n)) => {
                        set_note(&project_id, None);
                        notify(&app, "La verificación completa encontró fallos", &format!("{n} órdenes de verificación nuevas en la cola."));
                        emit(&app, &project_id);
                        continue;
                    }
                    Ok(Verification::Green) | Ok(Verification::Skipped) => {
                        if let Ok(conn) = db.0.lock() {
                            let _ = set_state(&conn, &project_id, "stopped");
                        }
                        let green = matches!(verdict, Ok(Verification::Green));
                        set_note(&project_id, Some(if green { "cola vacía · verificación completa en verde" } else { "cola vacía: todo entregado" }));
                        notify(&app, "La Factory terminó la cola", if green { "Todo entregado y la verificación completa en verde." } else { "No quedan órdenes por hacer en este proyecto." });
                    }
                    Ok(Verification::StillFailing(cmds)) => {
                        if let Ok(conn) = db.0.lock() {
                            let _ = set_state(&conn, &project_id, "stopped");
                        }
                        let why = format!("cola vacía, pero la verificación sigue fallando tras su orden: {}", cmds.join(" · "));
                        set_note(&project_id, Some(&why));
                        notify(&app, "La verificación completa sigue fallando", &why);
                    }
                    Err(e) => {
                        halt(&app, &project_id, &format!("no se pudo hacer la verificación completa: {e}"));
                    }
                }
                emit(&app, &project_id);
                break;
            }
        }
    }
    if let Ok(mut w) = WORKERS.lock() {
        if w.get(&project_id).is_some_and(|x| Arc::ptr_eq(x, &worker)) {
            w.remove(&project_id);
        }
    }
    emit(&app, &project_id);
}

/// Stops the loop on something it can't get past by waiting — an account
/// whose CLI can't be prepared — and says so, instead of leaving it «running».
fn halt<R: Runtime>(app: &AppHandle<R>, project_id: &str, why: &str) {
    if let Ok(conn) = app.state::<DbConnection>().0.lock() {
        let _ = set_state(&conn, project_id, "stopped");
    }
    set_note(project_id, Some(&format!("parada: {why}")));
    notify(app, "La Factory se paró", why);
    emit(app, project_id);
}

/// What the full verification found.
pub enum Verification {
    /// Nothing merged since the last one: nothing to verify.
    Skipped,
    Green,
    /// Failing commands became this many new orders.
    NewOrders(usize),
    /// These still fail, and already had their verification order.
    StillFailing(Vec<String>),
}

/// The last merge each project was verified after, so an empty queue isn't
/// verified twice for the same code.
static VERIFIED: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// Runs every command of the gate — testers and all the brain's `check:` —
/// on the base, in a worktree of its own. What fails becomes an order.
fn verify_all<R: Runtime>(app: &AppHandle<R>, project_id: &str) -> Result<Verification, String> {
    let db = app.state::<DbConnection>();
    let (project, brain, members, last_merge) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let project = super::store::get(&conn, project_id)?;
        let brain = super::brain::load(&conn, project_id, None)?.ok_or("sin brain sellado")?;
        let members = super::team::load(&conn, project_id)?;
        let last: Option<String> = conn
            .query_row("SELECT MAX(finished_at) FROM factory_runs WHERE project_id = ?1 AND status = 'merged'", params![project_id], |r| r.get(0))
            .ok()
            .flatten();
        (project, brain, members, last)
    };
    let Some(last_merge) = last_merge else { return Ok(Verification::Skipped) };
    if VERIFIED.lock().ok().and_then(|v| v.get(project_id).cloned()).as_deref() == Some(last_merge.as_str()) {
        return Ok(Verification::Skipped);
    }
    let repo = std::path::Path::new(&project.project_path);
    let status = super::setup::repo_status(repo);
    let base_branch = status.base_branch.clone().unwrap_or_else(|| "main".into());
    let env = crate::git_identity::git_env_for_project(app, project_id);
    let git = |dir: &std::path::Path, args: &[&str]| crate::commands::pull_requests::git_capture_with_env(dir, args, &env);
    let base = if status.remote_url.is_some() && git(repo, &["fetch", "origin", &base_branch]).is_ok() { format!("origin/{base_branch}") } else { base_branch };
    let dir = repo.join(".hangar").join("worktrees").join("factory-verify");
    if dir.join(".git").exists() {
        git(&dir, &["checkout", "--detach", "--force", &base])?;
    } else {
        std::fs::create_dir_all(dir.parent().expect("has a parent")).map_err(|e| e.to_string())?;
        git(repo, &["worktree", "add", "--detach", &dir.to_string_lossy(), &base])?;
    }
    let _ = super::env::install_deps(&dir);
    let commands = line::gate_commands(&members, &brain, &Default::default(), true);
    let results = line::run_gate(&dir, &commands, line::GateMode::Green);
    if let Ok(mut v) = VERIFIED.lock() {
        v.insert(project_id.to_string(), last_merge);
    }
    let failing: Vec<&line::GateResult> = results.iter().filter(|r| !r.ok).collect();
    if failing.is_empty() {
        return Ok(Verification::Green);
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    verification_orders(&conn, project_id, &orders::key_prefix(&project.project_name), &brain, &failing)
}

/// Turns failing commands into verification orders, one per command and never
/// twice: a command that already had its order is reported, not looped on.
pub fn verification_orders(conn: &Connection, project_id: &str, prefix: &str, brain: &super::brain::Sealed, failing: &[&line::GateResult]) -> Result<Verification, String> {
    let existing = orders::list(conn, project_id)?;
    let mut created = 0;
    let mut still = Vec::new();
    for r in failing {
        let title = format!("Verificación: `{}` falla", r.command);
        if existing.iter().any(|o| o.title == title) {
            still.push(r.command.clone());
            continue;
        }
        let rules: Vec<String> = r.name.split(", ").filter(|n| brain.rules.iter().any(|x| &x.id == n)).map(str::to_string).collect();
        orders::insert(
            conn,
            project_id,
            &orders::NewOrder {
                key: orders::next_key(conn, project_id, prefix)?,
                title,
                description: format!("La verificación completa de la base falla en `{}` ({}).\n\nSalida:\n{}", r.command, r.name, r.tail),
                criteria: vec![format!("`{}` pasa en verde en la base.", r.command)],
                origin: "verify".into(),
                rules,
                ..Default::default()
            },
        )?;
        created += 1;
    }
    Ok(if created > 0 { Verification::NewOrders(created) } else { Verification::StillFailing(still) })
}

struct Parts {
    git_env: Vec<(String, String)>,
    root: Option<std::path::PathBuf>,
}

impl Parts {
    /// The CLIs of every account the project's team and its running order use.
    fn runner(&self, db: &DbConnection, project_id: &str) -> Result<CliRunner, String> {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut ids: Vec<String> = super::team::load(&conn, project_id)?.into_iter().filter_map(|a| a.account_id).collect();
        if let Some(run) = line::active_run(&conn, project_id)? {
            ids.extend(run.team.iter().filter_map(|a| a.account_id.clone()));
            ids.extend(run.state.accounts.values().cloned());
        }
        ids.sort();
        ids.dedup();
        CliRunner::for_accounts(&conn, &ids, self.root.clone())
    }
}

fn context<R: Runtime>(app: &AppHandle<R>, project_id: &str) -> Parts {
    Parts { git_env: crate::git_identity::git_env_for_project(app, project_id), root: crate::app_data::root(app).ok() }
}

/// Advances one run until it delivers, waits or fails.
fn drive<R: Runtime>(app: &AppHandle<R>, project_id: &str, run_id: &str) {
    let db = app.state::<DbConnection>();
    let parts = context(app, project_id);
    let runner = match parts.runner(&db, project_id) {
        Ok(r) => r,
        Err(e) => {
            halt(app, project_id, &e);
            return;
        }
    };
    let notify_fn = |t: &str, b: &str| notify(app, t, b);
    let changed_fn = || emit(app, project_id);
    let ctx = Ctx { db: &db, runner: &runner, git_env: parts.git_env.clone(), root: parts.root.clone(), notify: &notify_fn, changed: &changed_fn };
    if let Err(e) = line::advance(&ctx, run_id) {
        set_note(project_id, Some(&e));
        if let Ok(conn) = db.0.lock() {
            if let Ok(mut run) = line::load_run(&conn, run_id) {
                run.status = "failed".into();
                run.error = Some(e.clone());
                run.finished_at = Some(Utc::now().to_rfc3339());
                let _ = line::save_run(&conn, &run);
            }
        }
        notify(app, "Una orden falló", &e);
    }
    emit(app, project_id);
}

fn sleep_or_wake<R: Runtime>(worker: &Worker, app: &AppHandle<R>, project_id: &str) {
    sleep_long(worker, app, project_id, Duration::from_secs(15));
}

/// Sleeps up to `total`, waking early on Resolve, ⏹ or a pause.
fn sleep_long<R: Runtime>(worker: &Worker, app: &AppHandle<R>, project_id: &str, total: Duration) {
    emit(app, project_id);
    let mut slept = Duration::ZERO;
    while slept < total {
        if worker.wake.swap(false, Ordering::SeqCst) {
            return;
        }
        std::thread::sleep(TICK);
        slept += TICK;
    }
}

/// Starts the project's worker unless one is running; wakes it otherwise.
fn ensure_worker<R: Runtime>(app: &AppHandle<R>, project_id: &str) {
    let mut workers = match WORKERS.lock() {
        Ok(w) => w,
        Err(_) => return,
    };
    if let Some(w) = workers.get(project_id) {
        w.wake.store(true, Ordering::SeqCst);
        return;
    }
    let worker = Arc::new(Worker { wake: AtomicBool::new(false) });
    workers.insert(project_id.to_string(), worker.clone());
    let app = app.clone();
    let id = project_id.to_string();
    std::thread::spawn(move || work(app, id, worker));
}

/// Picks up, after Hangar restarts, every project whose line was running.
pub fn resume_all<R: Runtime>(app: &AppHandle<R>) {
    let projects: Vec<String> = {
        let db = app.state::<DbConnection>();
        let Ok(conn) = db.0.lock() else { return };
        let Ok(mut stmt) = conn.prepare("SELECT project_id FROM factory_projects WHERE loop_state IN ('running', 'paused')") else { return };
        let rows = stmt.query_map([], |r| r.get::<_, String>(0)).map(|r| r.filter_map(Result::ok).collect()).unwrap_or_default();
        rows
    };
    for p in projects {
        ensure_worker(app, &p);
    }
}

// ---------------------------------------------------------------- commands

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopStatus {
    /// `stopped`, `running` or `paused`.
    pub state: String,
    /// Whether a worker is alive right now.
    pub working: bool,
    pub note: Option<String>,
    /// What the worker waits for before starting an order: `quota` or `merges`.
    pub wait: Option<String>,
}

fn status_of(conn: &Connection, project_id: &str) -> LoopStatus {
    let note = NOTES.lock().ok().and_then(|n| n.get(project_id).cloned());
    LoopStatus {
        state: loop_state(conn, project_id),
        working: WORKERS.lock().map(|w| w.contains_key(project_id)).unwrap_or(false),
        wait: note.as_ref().and_then(|(k, _)| k.map(str::to_string)),
        note: note.map(|(_, text)| text),
    }
}

/// «Empezar igualmente»: the next order starts even though the spec
/// writer's account looks nearly spent. The line still waits if it hits
/// the real limit.
#[tauri::command]
pub fn factory_start_anyway<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<LoopStatus, String> {
    if let Ok(mut s) = START_ANYWAY.lock() {
        s.insert(project_id.clone());
    }
    set_note(&project_id, None);
    ensure_worker(&app, &project_id);
    let db = app.state::<DbConnection>();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(status_of(&conn, &project_id))
}

#[tauri::command]
pub fn factory_loop_status(db: State<DbConnection>, project_id: String) -> Result<LoopStatus, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(status_of(&conn, &project_id))
}

/// ▶ Play. It prepares the project first — fixing what it can, as «Preparar»
/// does — and starts only when nothing is left that needs you.
#[tauri::command]
pub async fn factory_play<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<LoopStatus, String> {
    crate::blocking::run_blocking(move || {
        let git_env = crate::git_identity::git_env_for_project(&app, &project_id);
        let db = app.state::<DbConnection>();
        let checks = super::prepare::prepare(&db, &project_id, &git_env, true)?;
        if let Err(why) = super::prepare::blocking(&checks) {
            // In «En vivo» too, not only in a toast that goes away.
            set_wait(&project_id, Some("prepare"), Some(&format!("antes de empezar · {}", why.replace('\n', " · "))));
            emit(&app, &project_id);
            return Err(why);
        }
        emit(&app, &project_id);
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            if let Some(run) = line::active_run(&conn, &project_id)? {
                super::agent::clear_stop(&run.id);
            }
            set_state(&conn, &project_id, "running")?;
            conn.execute("UPDATE factory_projects SET setup_step = 6 WHERE project_id = ?1", params![project_id]).map_err(|e| e.to_string())?;
            set_note(&project_id, None);
        }
        ensure_worker(&app, &project_id);
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        Ok(status_of(&conn, &project_id))
    })
    .await?
}

/// ⏸ Pausa: the order at work finishes; no other starts.
#[tauri::command]
pub fn factory_pause(db: State<DbConnection>, project_id: String) -> Result<LoopStatus, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if loop_state(&conn, &project_id) == "running" {
        set_state(&conn, &project_id, "paused")?;
    }
    Ok(status_of(&conn, &project_id))
}

/// ⏹ Stop: now. The agent at work is killed; ▶ (or Reintentar) picks the
/// order up where it was, in the same conversation.
#[tauri::command]
pub fn factory_stop<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<LoopStatus, String> {
    let db = app.state::<DbConnection>();
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    set_state(&conn, &project_id, "stopped")?;
    if let Some(run) = line::active_run(&conn, &project_id)? {
        if run.status == "running" {
            // Kills the agent at work, or stops the line at the next stage.
            super::agent::stop(&run.id);
        } else if run.status == "waiting_limit" {
            let mut r = run;
            r.status = "stopped".into();
            line::save_run(&conn, &r)?;
        }
    }
    if let Some(w) = WORKERS.lock().ok().and_then(|w| w.get(&project_id).cloned()) {
        w.wake.store(true, Ordering::SeqCst);
    }
    set_note(&project_id, Some("parada · ▶ retoma la orden donde quedó"));
    drop(conn);
    emit(&app, &project_id);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(status_of(&conn, &project_id))
}

/// «Reintentar» on a failed order: its last run goes on at its stage, with
/// its rounds counted afresh; without a run it goes back to the queue.
pub fn retry_order(conn: &Connection, order_id: &str) -> Result<(), String> {
    let order = orders::get(conn, order_id)?;
    if !matches!(order.status.as_str(), "failed" | "stopped") {
        return Err(format!("{} no está fallida ni parada", order.key));
    }
    let last: Option<String> = conn.query_row("SELECT id FROM factory_runs WHERE order_id = ?1 ORDER BY started_at DESC LIMIT 1", params![order_id], |r| r.get(0)).ok();
    // A run that never got its worktree has no team to go on with: the order
    // just goes back to the queue, and that run stays as history.
    match last.and_then(|id| line::load_run(conn, &id).ok()).filter(|r| matches!(r.status.as_str(), "failed" | "stopped") && !r.team.is_empty()) {
        Some(mut run) => {
            run.round = 0;
            run.state.escalated = false;
            run.error = None;
            run.finished_at = None;
            run.status = "running".into();
            line::save_run(conn, &run)
        }
        None => orders::set_status(conn, order_id, "todo"),
    }
}

#[tauri::command]
pub fn factory_order_retry<R: Runtime>(app: AppHandle<R>, order_id: String) -> Result<(), String> {
    let project_id = {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let project: String = conn.query_row("SELECT project_id FROM factory_orders WHERE id = ?1", params![order_id], |r| r.get(0)).map_err(|_| "esa orden ya no existe".to_string())?;
        retry_order(&conn, &order_id)?;
        project
    };
    set_note(&project_id, None);
    emit(&app, &project_id);
    Ok(())
}

/// A person's answer to a ✋, a lack of credits or «Usar otra cuenta».
#[tauri::command]
pub fn factory_run_resolve<R: Runtime>(app: AppHandle<R>, run_id: String, action: String, note: Option<String>, account_id: Option<String>) -> Result<(), String> {
    let project_id = {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let run = line::resolve(&conn, &run_id, &action, note, account_id)?;
        if loop_state(&conn, &run.project_id) == "stopped" {
            // Answering is asking it to go on.
            set_state(&conn, &run.project_id, "running")?;
        }
        run.project_id
    };
    ensure_worker(&app, &project_id);
    emit(&app, &project_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_resumes_the_order_in_flight_before_taking_another() {
        let w = line::tests::world("autonomous", "local", "true");
        let conn = w.db.0.lock().unwrap();
        let other = orders::insert(&conn, "p1", &orders::NewOrder { key: "VET-3".into(), title: "Agenda".into(), origin: "trd".into(), ..Default::default() }).unwrap();
        assert_eq!(decide(&conn, "p1").unwrap(), Next::Start(w.order_id.clone()));
        drop(conn);

        let s = line::tests::Script::new();
        s.limit("builder");
        static N: fn(&str, &str) = |_, _| {};
        static C: fn() = || {};
        let ctx = Ctx { db: &w.db, runner: &s, git_env: crate::factory::setup::tests_identity(), root: None, notify: &N, changed: &C };
        let run = line::start(&ctx, "p1", &w.order_id).unwrap();
        let run = line::advance(&ctx, &run.id).unwrap();
        assert_eq!(run.status, "waiting_limit");

        let conn = w.db.0.lock().unwrap();
        assert_eq!(decide(&conn, "p1").unwrap(), Next::Resume(run.id.clone()), "the waiting order comes first, not VET-3");
        assert_eq!(failures_in_a_row(&conn, "p1").unwrap(), 0, "a limit is not a failure");
        drop(conn);
        line::advance(&ctx, &run.id).unwrap();
        let conn = w.db.0.lock().unwrap();
        assert_eq!(decide(&conn, "p1").unwrap(), Next::Start(other));
    }

    #[test]
    fn only_an_account_nearly_spent_or_benched_holds_an_order_back() {
        let w = line::tests::world("autonomous", "local", "true");
        let conn = w.db.0.lock().unwrap();
        conn.execute_batch(
            "ALTER TABLE accounts ADD COLUMN name TEXT DEFAULT 'David Personal 1';
             ALTER TABLE accounts ADD COLUMN status TEXT DEFAULT 'healthy';
             ALTER TABLE accounts ADD COLUMN scope TEXT DEFAULT 'personal';
             CREATE TABLE account_usage_samples (id TEXT PRIMARY KEY, account_id TEXT, session_pct REAL, weekly_pct REAL, context_pct REAL, sampled_at TEXT);",
        )
        .unwrap();
        assert!(room(&conn, "work", None).is_ok(), "never sampled: room");
        crate::quota::record_sample(&conn, "work", Some(63.0), Some(75.0), None).unwrap();
        assert!(room(&conn, "work", None).is_ok(), "75 % of the week isn't a reason to idle");
        crate::quota::record_sample(&conn, "work", Some(63.0), Some(96.0), None).unwrap();
        assert!(room(&conn, "work", None).unwrap_err().contains("96 % de la semana"));
        conn.execute("DELETE FROM account_usage_samples", []).unwrap();
        conn.execute("UPDATE accounts SET status = 'exhausted' WHERE id = 'work'", []).unwrap();
        assert!(room(&conn, "work", None).unwrap_err().contains("agotada"));
    }

    #[test]
    fn orders_waiting_on_a_failed_one_are_blocked_not_waiting_for_merges() {
        let w = line::tests::world("autonomous", "local", "true");
        let conn = w.db.0.lock().unwrap();
        orders::insert(&conn, "p1", &orders::NewOrder { key: "VET-3".into(), title: "x".into(), deps: vec!["VET-2".into()], origin: "trd".into(), ..Default::default() }).unwrap();
        orders::set_status(&conn, &w.order_id, "failed").unwrap();
        assert_eq!(decide(&conn, "p1").unwrap(), Next::Blocked(vec!["VET-2".into()]));
        retry_order(&conn, &w.order_id).unwrap();
        assert_eq!(decide(&conn, "p1").unwrap(), Next::Start(w.order_id.clone()), "no run yet: back to the queue");
    }

    #[test]
    fn a_folder_without_git_stops_the_project_and_leaves_the_order_alone() {
        let w = line::tests::world("autonomous", "local", "true");
        std::fs::remove_dir_all(w.repo.join(".git")).unwrap();
        let conn = w.db.0.lock().unwrap();
        assert!(line::ready(&conn, "p1").unwrap_err().contains("no es un repo git"));
        assert_eq!(orders::get(&conn, &w.order_id).unwrap().status, "todo");
    }

    #[test]
    fn retrying_a_failed_run_goes_on_at_its_stage_with_fresh_rounds() {
        let w = line::tests::world("autonomous", "local", "false");
        let s = line::tests::Script::new();
        static N: fn(&str, &str) = |_, _| {};
        static C: fn() = || {};
        let ctx = Ctx { db: &w.db, runner: &s, git_env: crate::factory::setup::tests_identity(), root: None, notify: &N, changed: &C };
        let run = line::start(&ctx, "p1", &w.order_id).unwrap();
        assert_eq!(line::advance(&ctx, &run.id).unwrap().status, "failed");
        let conn = w.db.0.lock().unwrap();
        retry_order(&conn, &w.order_id).unwrap();
        let again = line::load_run(&conn, &run.id).unwrap();
        assert_eq!((again.status.as_str(), again.round, again.state.escalated), ("running", 0, false));
        assert_eq!(decide(&conn, "p1").unwrap(), Next::Resume(run.id));
    }

    #[test]
    fn a_failing_full_verification_becomes_one_order_per_command_never_twice() {
        let w = line::tests::world("autonomous", "local", "true");
        let conn = w.db.0.lock().unwrap();
        let brain = crate::factory::brain::load(&conn, "p1", None).unwrap().unwrap();
        let e2e = line::GateResult { name: "ARQ-01, SEC-01".into(), command: "pnpm e2e".into(), ok: false, tail: "2 failed".into() };
        match verification_orders(&conn, "p1", "VET", &brain, &[&e2e]).unwrap() {
            Verification::NewOrders(1) => {}
            _ => panic!("one new order"),
        }
        let o = orders::list(&conn, "p1").unwrap().into_iter().find(|o| o.origin == "verify").unwrap();
        assert_eq!((o.title.as_str(), o.rules.clone()), ("Verificación: `pnpm e2e` falla", vec!["ARQ-01".to_string(), "SEC-01".to_string()]));
        match verification_orders(&conn, "p1", "VET", &brain, &[&e2e]).unwrap() {
            Verification::StillFailing(c) => assert_eq!(c, vec!["pnpm e2e"]),
            _ => panic!("not twice"),
        }
    }

    #[test]
    fn an_empty_queue_and_one_waiting_for_merges_are_told_apart() {
        let w = line::tests::world("autonomous", "mr", "true");
        let conn = w.db.0.lock().unwrap();
        orders::set_status(&conn, &w.order_id, "delivered").unwrap();
        assert_eq!(decide(&conn, "p1").unwrap(), Next::WaitMerges);
        orders::set_status(&conn, &w.order_id, "merged").unwrap();
        assert_eq!(decide(&conn, "p1").unwrap(), Next::Empty);
    }

    #[test]
    fn stop_halts_the_line_between_stages_and_play_goes_on_from_there() {
        let w = line::tests::world("autonomous", "local", "true");
        let s = line::tests::Script::new();
        static N: fn(&str, &str) = |_, _| {};
        static C: fn() = || {};
        let ctx = Ctx { db: &w.db, runner: &s, git_env: crate::factory::setup::tests_identity(), root: None, notify: &N, changed: &C };
        let run = line::start(&ctx, "p1", &w.order_id).unwrap();
        crate::factory::agent::stop(&run.id);
        let run = line::advance(&ctx, &run.id).unwrap();
        assert_eq!((run.status.as_str(), run.stage.as_str()), ("stopped", "spec"));
        assert!(s.calls.lock().unwrap().is_empty(), "nothing ran after ⏹");
        crate::factory::agent::clear_stop(&run.id);
        assert_eq!(line::advance(&ctx, &run.id).unwrap().status, "merged");
    }

    #[test]
    fn three_failures_in_a_row_trip_the_breaker() {
        let w = line::tests::world("autonomous", "local", "false");
        let s = line::tests::Script::new();
        static N: fn(&str, &str) = |_, _| {};
        static C: fn() = || {};
        let ctx = Ctx { db: &w.db, runner: &s, git_env: crate::factory::setup::tests_identity(), root: None, notify: &N, changed: &C };
        for i in 0..BREAKER {
            let id = if i == 0 {
                w.order_id.clone()
            } else {
                let conn = w.db.0.lock().unwrap();
                orders::insert(&conn, "p1", &orders::NewOrder { key: format!("VET-{}", 10 + i), title: "x".into(), origin: "trd".into(), ..Default::default() }).unwrap()
            };
            let run = line::start(&ctx, "p1", &id).unwrap();
            assert_eq!(line::advance(&ctx, &run.id).unwrap().status, "failed");
        }
        let conn = w.db.0.lock().unwrap();
        assert_eq!(failures_in_a_row(&conn, "p1").unwrap(), BREAKER);
    }
}
