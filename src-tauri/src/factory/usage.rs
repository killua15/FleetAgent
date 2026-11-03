//! The team's accounts' usage, kept current while the line works. Hangar's
//! own poller only reads accounts with a terminal open and only after turns
//! it recorded itself, so an account the Factory drives headless would show
//! whatever it last read. Here the line says when each task ended, its
//! account is read again, and what the task spent is written into its step.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};

use crate::db::DbConnection;

/// What Claude Code said about the account's limits with its last turn
/// (`rate_limit_event`): free, on every turn, but without a percentage.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RateNote {
    /// `allowed`, `allowed_warning` or `rejected`.
    pub status: String,
    /// `five_hour`, `seven_day`…
    pub window: Option<String>,
    pub resets_at: Option<DateTime<Utc>>,
}

static NOTES: Mutex<Option<HashMap<String, RateNote>>> = Mutex::new(None);
/// One lock per account: its reads go one after another, never two hidden
/// `claude` at once for the same account.
static ACCOUNT_LOCKS: Mutex<Option<HashMap<String, Arc<Mutex<()>>>>> = Mutex::new(None);

/// A task that just ended, to read its account after and write what it spent.
#[derive(Debug, Clone)]
pub struct TaskDone {
    pub project_id: String,
    pub run_id: String,
    pub step_id: String,
    pub account_id: String,
    /// The agent, as the activity names it.
    pub label: String,
    /// Its account's reading when the task started.
    pub before: Snapshot,
    /// Its turn's cost, in API-price dollars.
    pub cost_usd: f64,
}

/// Runs a task's read. Installed by the app; absent in tests, where tasks
/// end without reading anything.
type Hook = Box<dyn Fn(TaskDone) + Send + Sync>;
static HOOK: OnceLock<Hook> = OnceLock::new();

/// One `rate_limit_event` line of Claude Code's stream-json, if it is one.
pub fn parse_rate_limit(event: &Value) -> Option<RateNote> {
    if event.get("type").and_then(Value::as_str) != Some("rate_limit_event") {
        return None;
    }
    let info = event.get("rate_limit_info")?;
    Some(RateNote {
        status: info.get("status").and_then(Value::as_str)?.to_string(),
        window: info.get("rateLimitType").and_then(Value::as_str).map(str::to_string),
        resets_at: info.get("resetsAt").and_then(Value::as_i64).and_then(|s| Utc.timestamp_opt(s, 0).single()),
    })
}

pub fn note_rate_limit(account_id: &str, note: RateNote) {
    if let Ok(mut g) = NOTES.lock() {
        g.get_or_insert_with(HashMap::new).insert(account_id.to_string(), note);
    }
}

pub fn rate_note(account_id: &str) -> Option<RateNote> {
    NOTES.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(account_id).cloned()))
}

/// Runs `f` holding the account's own lock: reads of one account queue up.
pub fn serial<T>(account_id: &str, f: impl FnOnce() -> T) -> T {
    let lock = ACCOUNT_LOCKS.lock().map(|mut g| g.get_or_insert_with(HashMap::new).entry(account_id.to_string()).or_default().clone()).ok();
    let _held = lock.as_ref().map(|l| l.lock().unwrap_or_else(|e| e.into_inner()));
    f()
}

/// The line, when a task ended: read its account again — every task, every
/// time — and write what the task spent.
pub fn turn_done(task: TaskDone) {
    if let Some(hook) = HOOK.get() {
        hook(task);
    }
}

/// Wires the reads to the app: each task's on its own thread, one account's
/// in order, the database locked only to record, and the live view told.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    let _ = HOOK.set(Box::new(move |task| {
        let app = app.clone();
        std::thread::spawn(move || {
            serial(&task.account_id, || read(&app, &task.account_id, true));
            if let Some(db) = app.try_state::<DbConnection>() {
                if let Ok(conn) = db.0.lock() {
                    let after = snapshot(&conn, &task.account_id);
                    let _ = record_task(&conn, &task, &after);
                }
            }
            super::looping::emit_changed(&app, &task.project_id);
        });
    }));
}

/// What a task spent, from its account's reading before and after: `None`
/// for a window with no new reading, or one that renewed in between.
pub fn task_spent(before: &Snapshot, after: &Snapshot) -> (Option<f64>, Option<f64>) {
    let fresh = matches!((&before.at, &after.at), (Some(b), Some(a)) if a > b) || (before.at.is_none() && after.at.is_some());
    if !fresh {
        return (None, None);
    }
    Spent { before: before.clone(), after: Some(after.clone()) }.delta()
}

/// «≈ +3 % de la sesión de 5 h · +1 % de la semana · $0,42 equivalentes».
pub fn spent_words(session: Option<f64>, weekly: Option<f64>, cost_usd: f64) -> String {
    let mut parts = Vec::new();
    if let Some(s) = session {
        parts.push(format!("≈ +{s:.0} % de la sesión de 5 h"));
    }
    if let Some(w) = weekly {
        parts.push(format!("{}+{w:.0} % de la semana", if parts.is_empty() { "≈ " } else { "" }));
    }
    if parts.is_empty() {
        parts.push("uso sin lectura nueva".into());
    }
    parts.push(format!("${:.2} equivalentes", cost_usd).replace('.', ","));
    parts.join(" · ")
}

/// Writes what a task spent into its step and its activity.
pub fn record_task(conn: &Connection, task: &TaskDone, after: &Snapshot) -> Result<(), String> {
    let (session, weekly) = task_spent(&task.before, after);
    conn.execute("UPDATE factory_steps SET usage_session = ?2, usage_weekly = ?3 WHERE id = ?1", params![task.step_id, session, weekly]).map_err(|e| e.to_string())?;
    let station: String = conn.query_row("SELECT station FROM factory_steps WHERE id = ?1", params![task.step_id], |r| r.get(0)).unwrap_or_else(|_| "fab".into());
    conn.execute(
        "INSERT INTO factory_activity (run_id, step_id, station, at, kind, text, rule_id) VALUES (?1, ?2, ?3, ?4, 'cmd', ?5, NULL)",
        params![task.run_id, task.step_id, station, Utc::now().to_rfc3339(), format!("{} gastó {}", task.label, spent_words(session, weekly, task.cost_usd))],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// Reads one account's quota and records it — `/usage` for Claude, the
/// rollout on disk for Codex. Without the database lock while it reads.
pub fn read<R: Runtime>(app: &AppHandle<R>, account_id: &str, force: bool) -> bool {
    let Some(db) = app.try_state::<DbConnection>() else { return false };
    let row: Option<(String, String)> = db
        .0
        .lock()
        .ok()
        .and_then(|c| c.query_row("SELECT provider, config_dir_path FROM accounts WHERE id = ?1", params![account_id], |r| Ok((r.get(0)?, r.get(1)?))).ok());
    let Some((provider, dir)) = row else { return false };
    let reading = match provider.as_str() {
        "codex" => crate::usage_meter::codex_quota(&dir).map(|(s, w)| (s, w, None)),
        "claude" if std::path::Path::new(&dir).is_dir() => crate::usage_scraper::cached_usage_stats(app, account_id, &dir, force).ok().map(|s| (s.session_pct, s.weekly_pct, s.context_pct)),
        _ => None,
    };
    let Some((session, weekly, context)) = reading else { return false };
    db.0.lock().ok().is_some_and(|c| crate::quota::record_sample(&c, account_id, session, weekly, context).is_ok())
}

/// The last reading of an account: session and weekly %, and when.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub session: Option<f64>,
    pub weekly: Option<f64>,
    pub at: Option<String>,
}

pub fn snapshot(conn: &Connection, account_id: &str) -> Snapshot {
    conn.query_row(
        "SELECT session_pct, weekly_pct, sampled_at FROM account_usage_samples WHERE account_id = ?1 ORDER BY sampled_at DESC LIMIT 1",
        params![account_id],
        |r| Ok(Snapshot { session: r.get(0)?, weekly: r.get(1)?, at: r.get(2)? }),
    )
    .unwrap_or_default()
}

/// What an order cost an account: its usage before and after.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Spent {
    pub before: Snapshot,
    pub after: Option<Snapshot>,
}

impl Spent {
    /// Percentage points of the 5-hour session and of the week. `None` for a
    /// window without both readings, or one that reset in between.
    pub fn delta(&self) -> (Option<f64>, Option<f64>) {
        let Some(after) = &self.after else { return (None, None) };
        let d = |b: Option<f64>, a: Option<f64>| match (b, a) {
            (Some(b), Some(a)) if a >= b => Some(a - b),
            _ => None,
        };
        (d(self.before.session, after.session), d(self.before.weekly, after.weekly))
    }
}

/// The accounts a team works with.
pub fn team_accounts(team: &[super::team::Agent]) -> Vec<String> {
    let mut ids: Vec<String> = team.iter().filter(|a| a.active || a.required).filter_map(|a| a.account_id.clone()).collect();
    ids.sort();
    ids.dedup();
    ids
}

/// One account of the team as «En vivo» shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsage {
    pub account_id: String,
    pub name: String,
    pub provider: String,
    pub session: Option<f64>,
    pub weekly: Option<f64>,
    pub sampled_at: Option<String>,
    /// Claude Code's own word with its last turn.
    pub note: Option<RateNote>,
    /// When the 5-hour window renews, if anything said so.
    pub resets_at: Option<String>,
}

pub fn team_usage(conn: &Connection, project_id: &str, root: Option<&std::path::Path>) -> Result<Vec<AccountUsage>, String> {
    let team = super::team::load(conn, project_id)?;
    let mut out = Vec::new();
    for id in team_accounts(&team) {
        let (name, provider): (String, String) = conn.query_row("SELECT name, provider FROM accounts WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?))).map_err(|e| e.to_string())?;
        let snap = snapshot(conn, &id);
        let note = rate_note(&id);
        let resets_at = note
            .as_ref()
            .filter(|n| n.window.as_deref() != Some("seven_day"))
            .and_then(|n| n.resets_at)
            .or_else(|| root.and_then(|r| crate::auto_continue::latest_rate_limits(r, &id)).and_then(|l| crate::auto_continue::resume_at(&l, 0)))
            .filter(|at| *at > Utc::now())
            .map(|at| at.to_rfc3339());
        out.push(AccountUsage { account_id: id, name, provider, session: snap.session, weekly: snap.weekly, sampled_at: snap.at, note, resets_at });
    }
    Ok(out)
}

#[tauri::command]
pub async fn factory_usage<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<Vec<AccountUsage>, String> {
    let root = crate::app_data::root(&app).ok();
    crate::factory::with_db(app, move |conn| team_usage(conn, &project_id, root.as_deref())).await
}

/// ↻: reads every account of the team now.
#[tauri::command]
pub async fn factory_usage_refresh<R: Runtime>(app: AppHandle<R>, project_id: String) -> Result<Vec<AccountUsage>, String> {
    let accounts = {
        let id = project_id.clone();
        crate::factory::with_db(app.clone(), move |conn| Ok(team_accounts(&super::team::load(conn, &id)?))).await?
    };
    let reader = app.clone();
    crate::blocking::run_blocking(move || {
        for a in &accounts {
            read(&reader, a, true);
        }
    })
    .await?;
    factory_usage(app, project_id).await
}

/// When an order ends: every account of its team read again, and what the
/// order cost each one written into the run.
pub fn after_order<R: Runtime>(app: &AppHandle<R>, project_id: &str, run_id: &str) {
    let app = app.clone();
    let (project_id, run_id) = (project_id.to_string(), run_id.to_string());
    std::thread::spawn(move || {
        let accounts: Vec<String> = {
            let Some(db) = app.try_state::<DbConnection>() else { return };
            let Ok(conn) = db.0.lock() else { return };
            let Ok(run) = super::line::load_run(&conn, &run_id) else { return };
            run.state.usage.keys().cloned().collect()
        };
        for a in &accounts {
            read(&app, a, true);
        }
        if let Some(db) = app.try_state::<DbConnection>() {
            if let Ok(conn) = db.0.lock() {
                let _ = record_after(&conn, &run_id);
            }
        }
        super::looping::emit_changed(&app, &project_id);
    });
}

/// Writes each account's reading now as the run's «after», and the cost into
/// its activity.
pub fn record_after(conn: &Connection, run_id: &str) -> Result<(), String> {
    let mut run = super::line::load_run(conn, run_id)?;
    let mut said = Vec::new();
    for (account, spent) in run.state.usage.iter_mut() {
        spent.after = Some(snapshot(conn, account));
        let (s, w) = spent.delta();
        let name: String = conn.query_row("SELECT name FROM accounts WHERE id = ?1", params![account], |r| r.get(0)).unwrap_or_else(|_| account.clone());
        match (s, w) {
            (None, None) => {}
            _ => said.push(format!(
                "{name}: {}{}",
                s.map(|s| format!("≈ +{s:.0} % de la sesión de 5 h")).unwrap_or_default(),
                w.map(|w| format!("{}+{w:.0} % de la semana", if s.is_some() { " · " } else { "≈ " })).unwrap_or_default()
            )),
        }
    }
    super::line::save_run(conn, &run)?;
    if !said.is_empty() {
        conn.execute(
            "INSERT INTO factory_activity (run_id, step_id, station, at, kind, text, rule_id) VALUES (?1, NULL, 'ent', ?2, 'cmd', ?3, NULL)",
            params![run.id, Utc::now().to_rfc3339(), format!("uso de esta orden · {}", said.join(" · "))],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_codes_rate_limit_event_is_read() {
        let ev: Value = serde_json::from_str(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1793410800,"rateLimitType":"five_hour","overageStatus":"rejected","isUsingOverage":false},"session_id":"s"}"#).unwrap();
        let n = parse_rate_limit(&ev).unwrap();
        assert_eq!((n.status.as_str(), n.window.as_deref()), ("allowed_warning", Some("five_hour")));
        assert_eq!(n.resets_at.unwrap().timestamp(), 1793410800);
        assert!(parse_rate_limit(&serde_json::json!({"type":"result"})).is_none());
    }

    #[test]
    fn one_accounts_reads_go_one_after_another() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let at_once = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let acc = format!("acc-{}", uuid::Uuid::new_v4());
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let (at_once, most, acc) = (at_once.clone(), most.clone(), acc.clone());
                std::thread::spawn(move || {
                    serial(&acc, || {
                        let n = at_once.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(n, Ordering::SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        at_once.fetch_sub(1, Ordering::SeqCst);
                    })
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(most.load(Ordering::SeqCst), 1, "never two reads of one account at once");
    }

    #[test]
    fn a_task_spends_after_minus_before_only_with_a_new_reading() {
        let s = |session, weekly, at: &str| Snapshot { session, weekly, at: Some(at.into()) };
        let before = s(Some(40.0), Some(10.0), "2026-11-03T10:00:00Z");
        assert_eq!(task_spent(&before, &s(Some(43.0), Some(11.0), "2026-11-03T10:05:00Z")), (Some(3.0), Some(1.0)));
        assert_eq!(task_spent(&before, &before), (None, None), "/usage failed: the same reading");
        assert_eq!(task_spent(&before, &s(Some(2.0), Some(11.0), "2026-11-03T10:05:00Z")), (None, Some(1.0)), "the 5-hour window renewed");
        assert_eq!(spent_words(Some(3.0), Some(1.0), 0.42), "≈ +3 % de la sesión de 5 h · +1 % de la semana · $0,42 equivalentes");
        assert_eq!(spent_words(None, None, 0.0), "uso sin lectura nueva · $0,00 equivalentes");
    }

    fn samples(conn: &Connection) {
        conn.execute_batch("CREATE TABLE IF NOT EXISTS account_usage_samples (id TEXT PRIMARY KEY, account_id TEXT, session_pct REAL, weekly_pct REAL, context_pct REAL, sampled_at TEXT);").unwrap();
    }

    #[test]
    fn an_order_writes_what_it_cost_each_account() {
        use crate::factory::line::{self, tests::*};
        let w = world("autonomous", "local", "true");
        {
            let conn = w.db.0.lock().unwrap();
            samples(&conn);
            crate::quota::record_sample(&conn, "work", Some(40.0), Some(10.0), None).unwrap();
        }
        let s = Script::new();
        let c = ctx(&w, &s);
        let run = line::start(&c, "p1", &w.order_id).unwrap();
        assert_eq!(run.state.usage["work"].before.session, Some(40.0), "read as the order starts");
        let run = line::advance(&c, &run.id).unwrap();
        assert_eq!(run.status, "merged");
        let conn = w.db.0.lock().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        crate::quota::record_sample(&conn, "work", Some(52.0), Some(13.0), None).unwrap();
        record_after(&conn, &run.id).unwrap();
        let run = line::load_run(&conn, &run.id).unwrap();
        assert_eq!(run.state.usage["work"].delta(), (Some(12.0), Some(3.0)));
        let said: String = conn.query_row("SELECT text FROM factory_activity WHERE run_id = ?1 AND text LIKE 'uso de esta orden%'", params![run.id], |r| r.get(0)).unwrap();
        assert!(said.contains("+12 % de la sesión de 5 h") && said.contains("+3 % de la semana"), "{said}");
    }

    #[test]
    fn an_account_claude_code_refused_doesnt_start_an_order_until_it_renews() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE accounts (id TEXT PRIMARY KEY, name TEXT, provider TEXT, status TEXT, exhausted_at TEXT, config_dir_path TEXT, billing TEXT, created_at TEXT);
             INSERT INTO accounts VALUES ('acc-refused', 'Personal', 'claude', 'healthy', NULL, '/x', 'sub', '0');",
        )
        .unwrap();
        samples(&conn);
        crate::quota::record_sample(&conn, "acc-refused", Some(30.0), Some(5.0), None).unwrap();
        assert!(crate::factory::looping::room(&conn, "acc-refused", None).is_ok());
        note_rate_limit("acc-refused", RateNote { status: "rejected".into(), window: Some("five_hour".into()), resets_at: Some(Utc::now() + chrono::Duration::hours(1)) });
        assert!(crate::factory::looping::room(&conn, "acc-refused", None).unwrap_err().contains("llegó al límite"));
        note_rate_limit("acc-refused", RateNote { status: "allowed".into(), window: None, resets_at: None });
        assert!(crate::factory::looping::room(&conn, "acc-refused", None).is_ok());
    }

    #[test]
    fn each_task_writes_what_it_spent_and_the_order_adds_them_up() {
        use crate::factory::line::{self, tests::*};
        let w = world("autonomous", "local", "true");
        let s = Script::new();
        let c = ctx(&w, &s);
        let run = line::start(&c, "p1", &w.order_id).unwrap();
        let run = line::advance(&c, &run.id).unwrap();
        let conn = w.db.0.lock().unwrap();
        let steps: Vec<(String, String)> = {
            let mut st = conn.prepare("SELECT id, label FROM factory_steps WHERE run_id = ?1 AND agent_id IS NOT NULL ORDER BY started_at").unwrap();
            st.query_map(params![run.id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap().filter_map(Result::ok).collect()
        };
        assert!(steps.len() >= 3);
        let at = |m: u32| Some(format!("2026-11-03T10:{m:02}:00Z"));
        for (i, (step, label)) in steps.iter().take(2).enumerate() {
            let task = TaskDone {
                project_id: "p1".into(),
                run_id: run.id.clone(),
                step_id: step.clone(),
                account_id: "work".into(),
                label: label.clone(),
                before: Snapshot { session: Some(40.0 + i as f64 * 3.0), weekly: Some(10.0), at: at(i as u32) },
                cost_usd: 0.42,
            };
            let after = Snapshot { session: Some(43.0 + i as f64 * 3.0), weekly: Some(10.0 + (i as f64)), at: at(i as u32 + 1) };
            record_task(&conn, &task, &after).unwrap();
        }
        let said: String = conn.query_row("SELECT text FROM factory_activity WHERE step_id = ?1 AND text LIKE '%gastó%'", params![steps[0].0], |r| r.get(0)).unwrap();
        assert_eq!(said, format!("{} gastó ≈ +3 % de la sesión de 5 h · +0 % de la semana · $0,42 equivalentes", steps[0].1));
        let run = line::load_run(&conn, &run.id).unwrap();
        let total = crate::factory::view::order_usage(&conn, &run).unwrap();
        assert_eq!(total.len(), 1);
        assert_eq!((total[0].session, total[0].weekly), (Some(6.0), Some(1.0)), "the order is the sum of its tasks");
    }

    #[test]
    fn what_an_order_cost_is_after_minus_before_unless_a_window_renewed() {
        let s = |session, weekly| Snapshot { session, weekly, at: None };
        let spent = Spent { before: s(Some(40.0), Some(10.0)), after: Some(s(Some(52.0), Some(13.0))) };
        assert_eq!(spent.delta(), (Some(12.0), Some(3.0)));
        let renewed = Spent { before: s(Some(90.0), Some(10.0)), after: Some(s(Some(5.0), Some(11.0))) };
        assert_eq!(renewed.delta(), (None, Some(1.0)), "the 5-hour window renewed in between");
        assert_eq!(Spent { before: s(Some(1.0), None), after: None }.delta(), (None, None));
    }
}
