//! Turning `/usage` readings into a forecast.
//!
//! The poller has always scraped the real 5-hour and weekly percentages every
//! five minutes and kept them in a `HashMap` that each new sample overwrote.
//! One number answers "where am I"; a series answers **"how fast am I burning
//! and when do I run out"**, which is the question that actually changes what
//! you do next — and the one no other agent orchestrator can answer, because
//! none of them models accounts at all.
//!
//! Everything that decides anything here is a pure function over samples, so
//! the rules are testable without a database, a `claude`, or five hours of
//! waiting.

use crate::db::DbConnection;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

/// One `/usage` reading.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSample {
    pub sampled_at: DateTime<Utc>,
    pub pct: f64,
}

/// What the samples say about one quota window.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Forecast {
    /// Most recent reading, 0-100.
    pub pct: f64,
    /// Percentage points per hour. `None` when there isn't enough evidence —
    /// one sample, or no time between samples.
    pub burn_per_hour: Option<f64>,
    /// Hours until this window hits 100% at the current rate. `None` when the
    /// burn is zero or negative (idle), which is the common case and must not
    /// be rendered as "runs out now".
    pub hours_left: Option<f64>,
    /// How many samples the estimate rests on, so the UI can be honest about a
    /// forecast built from two readings ten minutes apart.
    pub samples: usize,
}

/// A window is considered to have reset when the percentage drops by more than
/// this. Small dips happen — the scrape is a lenient parse of a TUI, and
/// rounding moves the last digit — but a real reset is a cliff.
const RESET_DROP_PCT: f64 = 5.0;

/// Keeps only the samples since the last window reset.
///
/// Without this the forecast is nonsense every five hours: a 5-hour window
/// going 90% → 4% is not "you un-burned 86 points", it's a new window, and a
/// rate computed across that boundary comes out negative and hides a real
/// burn. Claude's windows are rolling, so this boundary shows up on its own
/// schedule and can't be predicted from the clock.
pub fn samples_since_reset(samples: &[UsageSample]) -> &[UsageSample] {
    let mut start = 0;
    for i in 1..samples.len() {
        if samples[i - 1].pct - samples[i].pct > RESET_DROP_PCT {
            start = i;
        }
    }
    &samples[start..]
}

/// Burn rate and time left for one window, from samples ordered oldest-first.
///
/// The rate is measured across the current window only (first to last sample
/// since the reset), not a least-squares fit: with a handful of noisy readings
/// from a parsed TUI, the endpoints are the honest signal and a regression
/// would just look more precise than the data is.
pub fn forecast(samples: &[UsageSample]) -> Option<Forecast> {
    let window = samples_since_reset(samples);
    let last = window.last()?;

    let (burn_per_hour, hours_left) = match window.first() {
        Some(first) if window.len() >= 2 => {
            let hours = (last.sampled_at - first.sampled_at).num_seconds() as f64 / 3600.0;
            if hours <= 0.0 {
                (None, None)
            } else {
                let burn = (last.pct - first.pct) / hours;
                // A flat or falling line means the account isn't being spent
                // right now. "Runs out in ∞" is not a forecast worth showing,
                // and a negative one would be a lie.
                let left = (burn > 0.0).then(|| ((100.0 - last.pct) / burn).max(0.0));
                (Some(burn), left)
            }
        }
        _ => (None, None),
    };

    Some(Forecast { pct: last.pct, burn_per_hour, hours_left, samples: window.len() })
}

/// Both windows of one account, plus who it is — what the fleet view lists.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuota {
    pub account_id: String,
    pub account_name: String,
    pub status: String,
    /// The rolling 5-hour window: the one you recover from by having lunch.
    pub session: Option<Forecast>,
    /// The weekly window: the one you don't recover from, and therefore the
    /// only one worth *planning* around.
    pub weekly: Option<Forecast>,
}

/// Records one reading. Called from the poller, which was already fetching it.
pub fn record_sample(conn: &Connection, account_id: &str, session_pct: Option<f64>, weekly_pct: Option<f64>, context_pct: Option<f64>) -> Result<(), String> {
    // A reading where every window came back empty says nothing about usage —
    // it says the scrape failed to parse. Storing it would put a hole in the
    // series that looks like data.
    if session_pct.is_none() && weekly_pct.is_none() {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO account_usage_samples (id, account_id, session_pct, weekly_pct, context_pct, sampled_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![Uuid::new_v4().to_string(), account_id, session_pct, weekly_pct, context_pct, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// How far back the forecast looks. A week covers the weekly window with room
/// to spare, and caps what one query has to read.
const LOOKBACK_HOURS: i64 = 24 * 8;

fn load_samples(conn: &Connection, account_id: &str, column: &str) -> Result<Vec<UsageSample>, String> {
    let since = (Utc::now() - chrono::Duration::hours(LOOKBACK_HOURS)).to_rfc3339();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT sampled_at, {column} FROM account_usage_samples
             WHERE account_id = ?1 AND {column} IS NOT NULL AND sampled_at >= ?2
             ORDER BY sampled_at"
        ))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![account_id, since], |row| {
            let at: String = row.get(0)?;
            let pct: f64 = row.get(1)?;
            Ok((at, pct))
        })
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for row in rows {
        let (at, pct) = row.map_err(|e| e.to_string())?;
        if let Ok(parsed) = DateTime::parse_from_rfc3339(&at) {
            out.push(UsageSample { sampled_at: parsed.with_timezone(&Utc), pct });
        }
    }
    Ok(out)
}

pub(crate) fn account_quotas(conn: &Connection) -> Result<Vec<AccountQuota>, String> {
    let mut stmt = conn
        .prepare("SELECT id, name, status FROM accounts WHERE provider = 'claude' ORDER BY scope, name")
        .map_err(|e| e.to_string())?;
    let accounts: Vec<(String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for (account_id, account_name, status) in accounts {
        out.push(AccountQuota {
            session: forecast(&load_samples(conn, &account_id, "session_pct")?),
            weekly: forecast(&load_samples(conn, &account_id, "weekly_pct")?),
            account_id,
            account_name,
            status,
        });
    }
    Ok(out)
}

/// Every account's quota picture — the fleet view, and the account chip's
/// "about two hours left".
#[tauri::command]
pub fn list_account_quotas(db: tauri::State<DbConnection>) -> Result<Vec<AccountQuota>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    account_quotas(&conn)
}

/// What a fan-out would cost, before launching it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FanOutProjection {
    pub account_id: String,
    pub account_name: String,
    /// Hours of headroom this account has right now, on whichever window runs
    /// out first.
    pub hours_left: Option<f64>,
    /// The window that limits it: `session`, `weekly`, or `none` when nothing
    /// is being burned fast enough to run out.
    pub limiting_window: String,
    /// True when this account is already benched — a branch aimed at it won't
    /// start at all.
    pub exhausted: bool,
}

/// Projects a fan-out across the accounts it would use.
///
/// The point of this is the thing that catches people out elsewhere: a fan-out
/// of five agents on **one** account spends that quota five times as fast, so
/// the same gesture that looks free in a demo empties an account in twenty
/// minutes. Hangar can spread it over N accounts — but only if you can see, at
/// the moment you launch, what each one has left.
#[tauri::command]
pub fn project_fan_out(db: tauri::State<DbConnection>, account_ids: Vec<String>) -> Result<Vec<FanOutProjection>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let quotas = account_quotas(&conn)?;

    let mut out = Vec::new();
    for account_id in account_ids {
        let Some(quota) = quotas.iter().find(|q| q.account_id == account_id) else { continue };
        let session_left = quota.session.as_ref().and_then(|f| f.hours_left);
        let weekly_left = quota.weekly.as_ref().and_then(|f| f.hours_left);
        let (hours_left, limiting_window) = match (session_left, weekly_left) {
            (Some(s), Some(w)) if w <= s => (Some(w), "weekly"),
            (Some(s), _) => (Some(s), "session"),
            (None, Some(w)) => (Some(w), "weekly"),
            (None, None) => (None, "none"),
        };
        out.push(FanOutProjection {
            account_id: quota.account_id.clone(),
            account_name: quota.account_name.clone(),
            hours_left,
            limiting_window: limiting_window.to_string(),
            exhausted: quota.status != "healthy",
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(minutes_ago: i64, pct: f64) -> UsageSample {
        UsageSample { sampled_at: Utc::now() - chrono::Duration::minutes(minutes_ago), pct }
    }

    #[test]
    fn one_sample_gives_a_reading_but_no_rate() {
        let f = forecast(&[at(0, 42.0)]).unwrap();
        assert_eq!(f.pct, 42.0);
        assert_eq!(f.burn_per_hour, None, "a single point has no slope");
        assert_eq!(f.hours_left, None);
    }

    #[test]
    fn a_steady_burn_projects_when_the_window_runs_out() {
        // 20% → 60% over two hours = 20 points/hour, 40 points left → 2 hours.
        let f = forecast(&[at(120, 20.0), at(60, 40.0), at(0, 60.0)]).unwrap();
        assert!((f.burn_per_hour.unwrap() - 20.0).abs() < 0.5, "got {:?}", f.burn_per_hour);
        assert!((f.hours_left.unwrap() - 2.0).abs() < 0.1, "got {:?}", f.hours_left);
        assert_eq!(f.samples, 3);
    }

    /// The case that makes a naive forecast useless every five hours: the
    /// rolling window resets and the percentage falls off a cliff. Measuring
    /// across that boundary gives a negative rate and hides a real burn.
    #[test]
    fn a_window_reset_starts_the_measurement_over_instead_of_going_negative() {
        let samples = vec![at(300, 70.0), at(240, 90.0), at(180, 4.0), at(120, 14.0), at(60, 24.0)];
        let f = forecast(&samples).unwrap();

        assert_eq!(f.samples, 3, "only the samples since the reset count");
        assert_eq!(f.pct, 24.0);
        assert!(f.burn_per_hour.unwrap() > 0.0, "the burn after the reset is real and positive");
        assert!((f.burn_per_hour.unwrap() - 10.0).abs() < 0.5, "got {:?}", f.burn_per_hour);
    }

    /// Rounding noise in a parsed TUI reading must not look like a reset, or
    /// every other sample would restart the series and there'd never be enough
    /// history to forecast from.
    #[test]
    fn a_small_dip_is_noise_not_a_reset() {
        let f = forecast(&[at(120, 40.0), at(60, 39.0), at(0, 45.0)]).unwrap();
        assert_eq!(f.samples, 3);
    }

    /// An idle account isn't running out. Reporting "0 hours left" for a flat
    /// line would send the user rotating away from a perfectly good account.
    #[test]
    fn an_idle_account_has_no_time_to_exhaustion() {
        let f = forecast(&[at(120, 30.0), at(60, 30.0), at(0, 30.0)]).unwrap();
        assert_eq!(f.burn_per_hour, Some(0.0));
        assert_eq!(f.hours_left, None, "flat means not running out, not running out now");
    }

    #[test]
    fn no_samples_means_no_forecast_at_all() {
        assert_eq!(forecast(&[]), None);
    }

    /// Already at the limit: zero hours left, not a negative number.
    #[test]
    fn an_account_at_the_limit_reports_no_time_left_rather_than_negative() {
        let f = forecast(&[at(60, 90.0), at(0, 100.0)]).unwrap();
        assert_eq!(f.hours_left, Some(0.0));
    }

    fn seed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Personal','personal','','','acc1','healthy','now','now')",
            [],
        )
        .unwrap();
        conn
    }

    /// A scrape that parsed nothing is not a usage of zero — storing it would
    /// put a hole in the series that later reads as a window reset.
    #[test]
    fn a_reading_with_no_windows_is_not_recorded() {
        let conn = seed();
        record_sample(&conn, "acc1", None, None, Some(12.0)).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM account_usage_samples", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);

        record_sample(&conn, "acc1", Some(30.0), None, None).unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM account_usage_samples", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1, "a partial reading with a real window is still data");
    }

    #[test]
    fn quotas_come_back_per_account_with_both_windows() {
        let conn = seed();
        let two_hours_ago = (Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        conn.execute(
            "INSERT INTO account_usage_samples (id, account_id, session_pct, weekly_pct, sampled_at) VALUES ('s1','acc1',20.0,10.0,?1)",
            params![two_hours_ago],
        )
        .unwrap();
        record_sample(&conn, "acc1", Some(60.0), Some(14.0), None).unwrap();

        let quotas = account_quotas(&conn).unwrap();
        assert_eq!(quotas.len(), 1);
        let session = quotas[0].session.as_ref().unwrap();
        assert_eq!(session.pct, 60.0);
        assert!(session.hours_left.unwrap() < 2.5, "burning 20 pts/h from 60% runs out in about two hours");

        let weekly = quotas[0].weekly.as_ref().unwrap();
        assert!(weekly.hours_left.unwrap() > session.hours_left.unwrap(), "the weekly window is burning far slower here");
    }
}
