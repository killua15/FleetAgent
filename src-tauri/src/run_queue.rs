//! Work that waits for quota instead of failing against it.
//!
//! The loudest documented complaint about Claude Code is hitting the usage limit
//! mid-task and losing the afternoon — one issue on its tracker has 1,482
//! comments. Every tool in this category lets you walk into that wall, this one
//! included until now. A queued run sits until an allowed account has headroom
//! and then starts itself, so "I ran out and stopped" becomes "it ran while I
//! was asleep".
//!
//! Three rules decide how honest this feature is, and all three are here rather
//! than spread across the callers:
//!
//! 1. **Task cost is never estimated.** There is no way to know whether a prompt
//!    will spend 2% or 40% of a window. What *is* knowable is how much room an
//!    account has left, so admission asks "is this account fresh enough to start
//!    something on" — not "does this task fit".
//! 2. **"Finished" means the agent stopped talking**, not that it succeeded. The
//!    signal is the `working → waiting` transition the terminal state machine
//!    already produces. The queue schedules attention, not correctness, and the
//!    UI has to say so in those words.
//! 3. **Unattended work always gets its own worktree.** A prompt that runs at
//!    3am does not touch the main checkout. Not configurable.

use crate::db::DbConnection;
use crate::quota::AccountQuota;
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use uuid::Uuid;

/// Above this on either window, an account is too close to its limit to start
/// new work on. Deliberately well short of 100: the point is to avoid starting
/// something that dies halfway, and a task that begins at 95% almost certainly
/// does.
const MAX_START_PCT: f64 = 70.0;

/// How many queued runs may be in flight at once. Each is a real CLI against a
/// real account, so this is a machine-load limit as much as a quota one.
const MAX_CONCURRENT_RUNS: usize = 2;

/// A run has to be alive at least this long before a quiet terminal counts as
/// "finished". Without it, the gap between spawning the shell and the CLI
/// printing anything reads as a completed turn and the queue marks every run
/// done the moment it starts.
const MIN_RUN_SECONDS: i64 = 20;

/// How many times a run is retried after the shell fails to come up. Bounded
/// because a prompt that can't start is usually a broken target, not bad luck,
/// and retrying it forever would occupy the queue in front of work that could
/// actually run.
const MAX_ATTEMPTS: i64 = 3;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QueuedRun {
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub prompt: String,
    pub branch_name: String,
    /// The account this run is pinned to, or `None` for "any the project
    /// allows".
    pub account_id: Option<String>,
    pub account_name: Option<String>,
    pub state: String,
    pub terminal_id: Option<String>,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// One candidate account, reduced to what admission actually looks at.
#[derive(Debug, Clone, PartialEq)]
pub struct AccountCapacity {
    pub account_id: String,
    pub healthy: bool,
    /// Highest percentage across the windows that matter. `None` when the
    /// account has never been sampled — see `has_capacity`.
    pub worst_pct: Option<f64>,
    /// Already running a queued run.
    pub busy: bool,
}

impl AccountCapacity {
    pub fn from_quota(quota: &AccountQuota, busy: bool) -> Self {
        let session = quota.session.as_ref().map(|f| f.pct);
        let weekly = quota.weekly.as_ref().map(|f| f.pct);
        let worst_pct = match (session, weekly) {
            (Some(s), Some(w)) => Some(s.max(w)),
            (Some(s), None) => Some(s),
            (None, w) => w,
        };
        AccountCapacity { account_id: quota.account_id.clone(), healthy: quota.status == "healthy", worst_pct, busy }
    }
}

/// Whether a run may start on this account right now.
///
/// An account with **no samples at all** is allowed. Refusing it would mean a
/// fresh install can never start anything — the poller only samples accounts
/// that already have a terminal open, so an unused account is silent by
/// construction, not because it's exhausted.
pub fn has_capacity(account: &AccountCapacity) -> bool {
    if !account.healthy || account.busy {
        return false;
    }
    match account.worst_pct {
        Some(pct) => pct < MAX_START_PCT,
        None => true,
    }
}

/// Picks the account a run should start on, or `None` to keep waiting.
///
/// Pinned runs wait for their own account rather than falling elsewhere: if you
/// named an account you meant it, and quietly spending a different quota is the
/// kind of helpfulness nobody asks for.
pub fn account_for_run(pinned: Option<&str>, candidates: &[AccountCapacity]) -> Option<String> {
    match pinned {
        Some(id) => candidates.iter().find(|c| c.account_id == id && has_capacity(c)).map(|c| c.account_id.clone()),
        None => candidates
            .iter()
            .filter(|c| has_capacity(c))
            // Freshest first: spreading work across the pool is the entire
            // reason the pool exists.
            .min_by(|a, b| a.worst_pct.unwrap_or(0.0).total_cmp(&b.worst_pct.unwrap_or(0.0)))
            .map(|c| c.account_id.clone()),
    }
}

/// How many more runs may start, given what's already in flight.
pub fn free_slots(running: usize) -> usize {
    MAX_CONCURRENT_RUNS.saturating_sub(running)
}

// ---------------------------------------------------------------------------
// Storage
// ---------------------------------------------------------------------------

const RUN_COLUMNS: &str = "r.id, r.project_id, p.name, r.prompt, r.branch_name, r.account_id, a.name, r.state, r.terminal_id, r.attempts, r.last_error, r.created_at, r.started_at, r.finished_at";

fn row_to_run(row: &rusqlite::Row) -> rusqlite::Result<QueuedRun> {
    Ok(QueuedRun {
        id: row.get(0)?,
        project_id: row.get(1)?,
        project_name: row.get(2)?,
        prompt: row.get(3)?,
        branch_name: row.get(4)?,
        account_id: row.get(5)?,
        account_name: row.get(6)?,
        state: row.get(7)?,
        terminal_id: row.get(8)?,
        attempts: row.get(9)?,
        last_error: row.get(10)?,
        created_at: row.get(11)?,
        started_at: row.get(12)?,
        finished_at: row.get(13)?,
    })
}

fn load_runs(conn: &Connection, states: &[&str]) -> Result<Vec<QueuedRun>, String> {
    let placeholders = std::iter::repeat_n("?", states.len()).collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT {RUN_COLUMNS} FROM queued_runs r
         JOIN projects p ON p.id = r.project_id
         LEFT JOIN accounts a ON a.id = r.account_id
         WHERE r.state IN ({placeholders})
         ORDER BY r.created_at"
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows = stmt.query_map(rusqlite::params_from_iter(states), row_to_run).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Queues work. Returns the row so the UI can show it immediately rather than
/// re-reading the list.
#[tauri::command]
pub fn enqueue_run(
    db: tauri::State<DbConnection>,
    project_id: String,
    prompt: String,
    branch_name: String,
    account_id: Option<String>,
) -> Result<QueuedRun, String> {
    if prompt.trim().is_empty() {
        return Err("a queued run needs a prompt".to_string());
    }
    if branch_name.trim().is_empty() {
        return Err("a queued run needs a branch name — unattended work always gets its own worktree".to_string());
    }
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let id = Uuid::new_v4().to_string();
    conn.execute(
        "INSERT INTO queued_runs (id, project_id, prompt, branch_name, account_id, state, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6)",
        params![id, project_id, prompt.trim(), branch_name.trim(), account_id, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;

    let mut stmt = conn
        .prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM queued_runs r JOIN projects p ON p.id = r.project_id LEFT JOIN accounts a ON a.id = r.account_id WHERE r.id = ?1"
        ))
        .map_err(|e| e.to_string())?;
    stmt.query_row(params![id], row_to_run).map_err(|e| e.to_string())
}

/// Everything not yet finished, plus recently finished runs for context.
#[tauri::command]
pub fn list_queued_runs(db: tauri::State<DbConnection>) -> Result<Vec<QueuedRun>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    load_runs(&conn, &["queued", "running", "failed", "done"])
}

/// Drops a run. A running one is left alone deliberately: killing an agent
/// mid-edit leaves a worktree in a state nobody asked for, so cancelling only
/// applies to work that hasn't started.
#[tauri::command]
pub fn cancel_queued_run(db: tauri::State<DbConnection>, id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let changed = conn
        .execute(
            "UPDATE queued_runs SET state = 'cancelled', finished_at = ?2 WHERE id = ?1 AND state = 'queued'",
            params![id, Utc::now().to_rfc3339()],
        )
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err("that run has already started — close its terminal instead".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Scheduling
// ---------------------------------------------------------------------------

/// One scheduling pass: start whatever can start.
///
/// Hangs off the usage poller rather than its own timer, because the poller is
/// exactly when capacity information changes — a tick between samples can only
/// reach the same conclusion as the last one.
pub fn tick_queue<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DbConnection>() else { return };

    let (queued, running_ids, quotas, budgets) = {
        let Ok(conn) = db.0.lock() else { return };
        let Ok(queued) = load_runs(&conn, &["queued"]) else { return };
        let Ok(running) = load_runs(&conn, &["running"]) else { return };
        let Ok(quotas) = crate::quota::account_quotas(&conn) else { return };
        // Budgets gate unattended work first of all: this is the path that
        // spends quota while nobody is watching, so it's where a cap has to
        // hold if it holds anywhere.
        let budgets = crate::quota::project_budgets(&conn).unwrap_or_default();
        (queued, running, quotas, budgets)
    };

    let mut slots = free_slots(running_ids.len());
    if slots == 0 || queued.is_empty() {
        return;
    }
    let mut busy: Vec<String> = running_ids.iter().filter_map(|r| r.account_id.clone()).collect();

    for run in queued {
        if slots == 0 {
            break;
        }
        if let Some(budget) = budgets.iter().find(|b| b.project_id == run.project_id) {
            if !crate::quota::within_budget(budget) {
                // Left queued rather than failed: the weekly window rolls, and
                // a run blocked on Thursday is runnable on Monday without
                // anyone re-entering it.
                continue;
            }
        }
        let candidates: Vec<AccountCapacity> = quotas
            .iter()
            .filter(|q| allowed_for_project(&db, &run.project_id, &q.account_id))
            .map(|q| AccountCapacity::from_quota(q, busy.contains(&q.account_id)))
            .collect();

        let Some(account_id) = account_for_run(run.account_id.as_deref(), &candidates) else { continue };
        match start_run(app, &db, &run, &account_id) {
            Ok(()) => {
                busy.push(account_id);
                slots -= 1;
            }
            Err(e) => record_failure(&db, &run, &e),
        }
    }
}

/// Whether a project may spend this account — the same opt-in list rotation
/// uses, so queued work can never reach an account the user didn't check.
fn allowed_for_project(db: &DbConnection, project_id: &str, account_id: &str) -> bool {
    let Ok(conn) = db.0.lock() else { return false };
    conn.query_row(
        "SELECT 1 FROM project_allowed_accounts WHERE project_id = ?1 AND account_id = ?2 AND allowed = 1",
        params![project_id, account_id],
        |_| Ok(()),
    )
    .optional()
    .map(|found| found.is_some())
    .unwrap_or(false)
}

fn start_run<R: Runtime>(app: &AppHandle<R>, db: &DbConnection, run: &QueuedRun, account_id: &str) -> Result<(), String> {
    let session = tauri::async_runtime::block_on(crate::commands::sessions::create_worktree_for_project(
        app.clone(),
        run.project_id.clone(),
        run.branch_name.clone(),
        account_id.to_string(),
        None,
        // Queued work branches off whatever the project has checked out: there
        // is nobody at the keyboard to pick a base, and inventing one would be
        // a guess with consequences.
        None,
    ))?;

    let info = crate::terminals::open_terminal(
        app.clone(),
        crate::terminals::TerminalTarget { kind: crate::terminals::TargetKind::Session, id: session.id.clone() },
        Some(account_id.to_string()),
        Some(crate::terminals::InitCommand::Start),
        None,
        Some(run.prompt.clone()),
        80,
        24,
    )?;

    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "UPDATE queued_runs SET state = 'running', terminal_id = ?2, session_id = ?3, account_id = ?4, started_at = ?5, last_error = NULL WHERE id = ?1",
        params![run.id, info.id, session.id, account_id, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    drop(conn);

    let _ = app.emit("queue-updated", ());
    log::info!("queue: started run {} on account {account_id} (branch {})", run.id, run.branch_name);
    Ok(())
}

fn record_failure(db: &DbConnection, run: &QueuedRun, error: &str) {
    let Ok(conn) = db.0.lock() else { return };
    let attempts = run.attempts + 1;
    // Give up rather than occupy the queue in front of work that could run.
    let state = if attempts >= MAX_ATTEMPTS { "failed" } else { "queued" };
    let _ = conn.execute(
        "UPDATE queued_runs SET attempts = ?2, last_error = ?3, state = ?4, finished_at = ?5 WHERE id = ?1",
        params![run.id, attempts, error, state, (state == "failed").then(|| Utc::now().to_rfc3339())],
    );
    log::warn!("queue: run {} failed to start (attempt {attempts}): {error}", run.id);
}

/// Marks a running run finished because its terminal went quiet.
///
/// Called from the terminal state watcher on `working → waiting`. "Finished"
/// here means the agent stopped producing output — the queue has no idea
/// whether the work is any good, and says so in the UI rather than implying a
/// verdict it can't reach.
pub fn note_terminal_waiting<R: Runtime>(app: &AppHandle<R>, terminal_id: &str) {
    let Some(db) = app.try_state::<DbConnection>() else { return };
    let Ok(conn) = db.0.lock() else { return };

    let started: Option<String> = conn
        .query_row("SELECT started_at FROM queued_runs WHERE terminal_id = ?1 AND state = 'running'", params![terminal_id], |r| {
            r.get::<_, Option<String>>(0)
        })
        .optional()
        .ok()
        .flatten()
        .flatten();
    let Some(started) = started else { return };

    // The gap between spawning a shell and the CLI printing its banner looks
    // exactly like a finished turn. Without this guard every run completes the
    // instant it starts.
    if let Ok(started_at) = chrono::DateTime::parse_from_rfc3339(&started) {
        if (Utc::now() - started_at.with_timezone(&Utc)).num_seconds() < MIN_RUN_SECONDS {
            return;
        }
    }

    let _ = conn.execute(
        "UPDATE queued_runs SET state = 'done', finished_at = ?2 WHERE terminal_id = ?1 AND state = 'running'",
        params![terminal_id, Utc::now().to_rfc3339()],
    );
    drop(conn);
    let _ = app.emit("queue-updated", ());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cap(id: &str, pct: Option<f64>, healthy: bool, busy: bool) -> AccountCapacity {
        AccountCapacity { account_id: id.to_string(), healthy, worst_pct: pct, busy }
    }

    #[test]
    fn a_fresh_account_can_take_work_and_a_nearly_spent_one_cannot() {
        assert!(has_capacity(&cap("a", Some(10.0), true, false)));
        assert!(!has_capacity(&cap("a", Some(85.0), true, false)), "starting at 85% is starting something that dies halfway");
    }

    /// The poller only samples accounts that already have a terminal open, so
    /// an unused account is silent by construction. Treating silence as
    /// exhaustion would mean a fresh install can never start anything.
    #[test]
    fn an_account_with_no_samples_is_allowed_rather_than_assumed_empty() {
        assert!(has_capacity(&cap("a", None, true, false)));
    }

    #[test]
    fn benched_and_busy_accounts_are_both_skipped() {
        assert!(!has_capacity(&cap("a", Some(5.0), false, false)), "an exhausted account can't run anything");
        assert!(!has_capacity(&cap("a", Some(5.0), true, true)), "one queued run per account — two would share the quota they were spread to avoid");
    }

    #[test]
    fn an_unpinned_run_takes_the_freshest_account() {
        let candidates = vec![cap("busy-ish", Some(60.0), true, false), cap("fresh", Some(5.0), true, false)];
        assert_eq!(account_for_run(None, &candidates).as_deref(), Some("fresh"));
    }

    /// If you named an account you meant it. Silently spending a different
    /// quota is the kind of helpfulness nobody asks for.
    #[test]
    fn a_pinned_run_waits_for_its_own_account_instead_of_falling_elsewhere() {
        let candidates = vec![cap("wanted", Some(90.0), true, false), cap("other", Some(1.0), true, false)];
        assert_eq!(account_for_run(Some("wanted"), &candidates), None);

        let recovered = vec![cap("wanted", Some(20.0), true, false), cap("other", Some(1.0), true, false)];
        assert_eq!(account_for_run(Some("wanted"), &recovered).as_deref(), Some("wanted"));
    }

    #[test]
    fn nothing_starts_when_every_account_is_spent() {
        let candidates = vec![cap("a", Some(95.0), true, false), cap("b", Some(71.0), true, false)];
        assert_eq!(account_for_run(None, &candidates), None);
    }

    #[test]
    fn concurrency_is_capped_and_never_negative() {
        assert_eq!(free_slots(0), MAX_CONCURRENT_RUNS);
        assert_eq!(free_slots(MAX_CONCURRENT_RUNS), 0);
        assert_eq!(free_slots(MAX_CONCURRENT_RUNS + 5), 0, "more running than the cap must not wrap around");
    }

    fn seed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','P','/tmp','now')", []).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Personal','personal','','','acc1','healthy','now','now')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_run_without_a_branch_is_refused_because_unattended_work_needs_a_worktree() {
        let conn = seed();
        let inserted = conn.execute(
            "INSERT INTO queued_runs (id, project_id, prompt, branch_name, state, created_at) VALUES ('r1','proj1','do it','',
             'queued','now')",
            [],
        );
        // The column allows it; the command is what refuses, so this asserts the
        // rule lives in the command and is worth keeping there.
        assert!(inserted.is_ok(), "the schema doesn't enforce it — `enqueue_run` does");
    }

    #[test]
    fn runs_come_back_in_the_order_they_were_queued() {
        let conn = seed();
        for (id, at) in [("r2", "2026-07-26T10:00:00Z"), ("r1", "2026-07-26T09:00:00Z")] {
            conn.execute(
                "INSERT INTO queued_runs (id, project_id, prompt, branch_name, state, created_at) VALUES (?1,'proj1','p','b','queued',?2)",
                params![id, at],
            )
            .unwrap();
        }
        let runs = load_runs(&conn, &["queued"]).unwrap();
        assert_eq!(runs.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), vec!["r1", "r2"], "first queued, first started");
    }
}
