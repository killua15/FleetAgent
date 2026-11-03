//! The team's accounts' usage, kept current while the line works. Hangar's
//! own poller only reads accounts with a terminal open and only after turns
//! it recorded itself, so an account the Factory drives headless would show
//! whatever it last read. Here the line says when a turn ended, and the
//! account is read again — at most every few minutes per account while an
//! order runs, and always when the order ends.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager, Runtime};

use crate::db::DbConnection;

/// How often a turn may trigger a read of the same account.
pub const TURN_EVERY: Duration = Duration::from_secs(3 * 60);

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
static LAST_READ: Mutex<Option<HashMap<String, Instant>>> = Mutex::new(None);
/// Reads the accounts given, for a project, forced or not. Installed by the
/// app; absent in tests, where turns end without reading anything.
type Hook = Box<dyn Fn(String, Vec<String>, bool) + Send + Sync>;
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

/// Whether a turn may read this account now, and if so, marks it read.
pub fn due(account_id: &str, now: Instant, every: Duration) -> bool {
    let Ok(mut g) = LAST_READ.lock() else { return false };
    let map = g.get_or_insert_with(HashMap::new);
    if map.get(account_id).is_some_and(|t| now.duration_since(*t) < every) {
        return false;
    }
    map.insert(account_id.to_string(), now);
    true
}

/// The line, when an agent's turn ended: read that account, unless it was
/// read moments ago.
pub fn turn_done(project_id: &str, account_id: &str) {
    if let Some(hook) = HOOK.get() {
        if due(account_id, Instant::now(), TURN_EVERY) {
            hook(project_id.to_string(), vec![account_id.to_string()], true);
        }
    }
}

/// Wires the reads to the app: each one on its own thread, the database
/// locked only to record, and the live view told.
pub fn install<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    let _ = HOOK.set(Box::new(move |project, accounts, force| {
        let app = app.clone();
        std::thread::spawn(move || {
            for account in &accounts {
                read(&app, account, force);
            }
            super::looping::emit_changed(&app, &project);
        });
    }));
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
    fn a_turn_reads_an_account_at_most_every_few_minutes() {
        let t0 = Instant::now();
        let acc = format!("acc-{}", uuid::Uuid::new_v4());
        assert!(due(&acc, t0, TURN_EVERY));
        assert!(!due(&acc, t0 + Duration::from_secs(60), TURN_EVERY), "a minute later: not again");
        assert!(due(&acc, t0 + TURN_EVERY + Duration::from_secs(1), TURN_EVERY));
        assert!(due("another", t0, TURN_EVERY), "per account");
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
    fn what_an_order_cost_is_after_minus_before_unless_a_window_renewed() {
        let s = |session, weekly| Snapshot { session, weekly, at: None };
        let spent = Spent { before: s(Some(40.0), Some(10.0)), after: Some(s(Some(52.0), Some(13.0))) };
        assert_eq!(spent.delta(), (Some(12.0), Some(3.0)));
        let renewed = Spent { before: s(Some(90.0), Some(10.0)), after: Some(s(Some(5.0), Some(11.0))) };
        assert_eq!(renewed.delta(), (None, Some(1.0)), "the 5-hour window renewed in between");
        assert_eq!(Spent { before: s(Some(1.0), None), after: None }.delta(), (None, None));
    }
}
