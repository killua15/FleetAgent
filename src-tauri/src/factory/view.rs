//! What the live view and the Entregas tab read: the order at work with its
//! steps, activity and decisions; today's orders for the timeline; and every
//! delivery with its rounds, cited rules and cost.

use std::collections::HashMap;

use chrono::{DateTime, Local, Utc};
use rusqlite::{params, Connection};
use serde::Serialize;


use super::line::{self, Run};
use super::orders::{self, Order};


#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepRow {
    pub id: String,
    pub run_id: String,
    pub agent_id: Option<String>,
    pub label: String,
    pub model: String,
    pub station: String,
    /// `running`, `done`, `failed`, `wait`, `human`, `skipped`, `stopped`.
    pub status: String,
    pub round: i64,
    pub note: Option<String>,
    pub cost_usd: f64,
    pub started_at: String,
    pub finished_at: Option<String>,
    /// What the step spent of its account: points of the 5-hour session and
    /// of the week (`None`: not read yet, or no new reading).
    pub usage_session: Option<f64>,
    pub usage_weekly: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityRow {
    pub id: i64,
    pub step_id: Option<String>,
    pub station: String,
    pub at: String,
    pub kind: String,
    pub text: String,
    pub rule_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionRow {
    pub step_id: Option<String>,
    pub station: String,
    pub at: String,
    pub rule_id: Option<String>,
    pub text: String,
}

/// One order of today, for the «Hoy» timeline.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayRow {
    pub order_key: String,
    pub title: String,
    pub status: String,
    pub resume_at: Option<String>,
    pub steps: Vec<StepRow>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Live {
    pub run: Option<Run>,
    pub order: Option<Order>,
    pub steps: Vec<StepRow>,
    pub activity: Vec<ActivityRow>,
    pub decisions: Vec<DecisionRow>,
    pub today: Vec<DayRow>,
    /// Station → average seconds a step took, from finished orders: what the
    /// timeline estimates the rest with.
    pub averages: HashMap<String, f64>,
    /// What the order in view has spent so far, per account.
    pub usage: Vec<UsageSpent>,
}

fn steps_of(conn: &Connection, run_id: &str) -> Result<Vec<StepRow>, String> {
    let mut stmt = conn
        .prepare("SELECT id, run_id, agent_id, label, model, station, status, round, note, cost_usd, started_at, finished_at, usage_session, usage_weekly FROM factory_steps WHERE run_id = ?1 ORDER BY started_at")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![run_id], |r| {
            Ok(StepRow {
                id: r.get(0)?,
                run_id: r.get(1)?,
                agent_id: r.get(2)?,
                label: r.get(3)?,
                model: r.get(4)?,
                station: r.get(5)?,
                status: r.get(6)?,
                round: r.get(7)?,
                note: r.get(8)?,
                cost_usd: r.get(9)?,
                started_at: r.get(10)?,
                finished_at: r.get(11)?,
                usage_session: r.get(12)?,
                usage_weekly: r.get(13)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

fn averages(conn: &Connection, project_id: &str) -> Result<HashMap<String, f64>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT s.station, s.started_at, s.finished_at FROM factory_steps s JOIN factory_runs r ON r.id = s.run_id
              WHERE r.project_id = ?1 AND s.status = 'done' AND s.finished_at IS NOT NULL ORDER BY s.started_at DESC LIMIT 400",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![project_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))).map_err(|e| e.to_string())?;
    let mut sums: HashMap<String, (f64, f64)> = HashMap::new();
    for (station, a, b) in rows.filter_map(Result::ok) {
        let (Ok(a), Ok(b)) = (DateTime::parse_from_rfc3339(&a), DateTime::parse_from_rfc3339(&b)) else { continue };
        let e = sums.entry(station).or_default();
        e.0 += (b - a).num_seconds().max(0) as f64;
        e.1 += 1.0;
    }
    Ok(sums.into_iter().map(|(k, (s, n))| (k, s / n)).collect())
}

/// The project's orders by id, read once — not once per run.
fn orders_by_id(conn: &Connection, project_id: &str) -> Result<HashMap<String, Order>, String> {
    Ok(orders::list(conn, project_id)?.into_iter().map(|o| (o.id.clone(), o)).collect())
}

pub fn live(conn: &Connection, project_id: &str) -> Result<Live, String> {
    let by_id = orders_by_id(conn, project_id)?;
    let run = match line::active_run(conn, project_id)? {
        Some(r) => Some(r),
        None => line::runs(conn, project_id, 1)?.into_iter().next(),
    };
    let (order, steps, activity, decisions) = match &run {
        Some(r) => {
            let mut stmt = conn
                .prepare("SELECT id, step_id, station, at, kind, text, rule_id FROM factory_activity WHERE run_id = ?1 ORDER BY id DESC LIMIT 100")
                .map_err(|e| e.to_string())?;
            let mut activity: Vec<ActivityRow> = stmt
                .query_map(params![r.id], |x| Ok(ActivityRow { id: x.get(0)?, step_id: x.get(1)?, station: x.get(2)?, at: x.get(3)?, kind: x.get(4)?, text: x.get(5)?, rule_id: x.get(6)? }))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .collect();
            activity.reverse();
            let mut stmt = conn.prepare("SELECT step_id, station, at, rule_id, text FROM factory_decisions WHERE run_id = ?1 ORDER BY at").map_err(|e| e.to_string())?;
            let decisions = stmt
                .query_map(params![r.id], |x| Ok(DecisionRow { step_id: x.get(0)?, station: x.get(1)?, at: x.get(2)?, rule_id: x.get(3)?, text: x.get(4)? }))
                .map_err(|e| e.to_string())?
                .filter_map(Result::ok)
                .collect();
            (by_id.get(&r.order_id).cloned(), steps_of(conn, &r.id)?, activity, decisions)
        }
        None => (None, Vec::new(), Vec::new(), Vec::new()),
    };

    // Today, in local time: every order that had a step since midnight.
    let midnight = Local::now().date_naive().and_hms_opt(0, 0, 0).and_then(|t| t.and_local_timezone(Local).single()).map(|t| t.with_timezone(&Utc).to_rfc3339()).unwrap_or_default();
    let mut today = Vec::new();
    for r in line::runs(conn, project_id, 50)? {
        if r.finished_at.as_deref().is_some_and(|f| f < midnight.as_str()) {
            continue;
        }
        let Some(o) = by_id.get(&r.order_id) else { continue };
        today.push(DayRow { order_key: o.key.clone(), title: o.title.clone(), status: r.status.clone(), resume_at: r.resume_at.clone(), steps: steps_of(conn, &r.id)? });
    }
    today.reverse();
    let usage = match &run {
        Some(r) => order_usage(conn, r)?,
        None => Vec::new(),
    };
    Ok(Live { run, order, steps, activity, decisions, today, averages: averages(conn, project_id)?, usage })
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Delivery {
    pub run_id: String,
    pub order_key: String,
    pub title: String,
    pub status: String,
    pub stage: String,
    pub mr_url: Option<String>,
    pub rounds: i64,
    /// The rules the round-trips were for (`SEC-01`).
    pub round_rules: Vec<String>,
    pub rules_cited: i64,
    pub cost_usd: f64,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    /// What the order cost each account of its team, in usage points.
    pub usage: Vec<UsageSpent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSpent {
    pub account: String,
    /// Points of the 5-hour session; `None` if unknown or it renewed.
    pub session: Option<f64>,
    pub weekly: Option<f64>,
}

/// What an order spent of each account: the sum of its tasks — what the
/// line read after each one — or, for an order from before tasks were
/// read, its accounts' usage before and after.
pub fn order_usage(conn: &Connection, run: &line::Run) -> Result<Vec<UsageSpent>, String> {
    let account_of = |agent: &str| run.state.accounts.get(agent).cloned().or_else(|| run.team.iter().find(|a| a.id == agent).and_then(|a| a.account_id.clone()));
    let mut stmt = conn.prepare("SELECT agent_id, usage_session, usage_weekly FROM factory_steps WHERE run_id = ?1 AND agent_id IS NOT NULL").map_err(|e| e.to_string())?;
    let rows: Vec<(String, Option<f64>, Option<f64>)> = stmt.query_map(params![run.id], |x| Ok((x.get(0)?, x.get(1)?, x.get(2)?))).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
    let mut by: std::collections::BTreeMap<String, (Option<f64>, Option<f64>)> = std::collections::BTreeMap::new();
    for (agent, s, w) in rows {
        let Some(acc) = account_of(&agent) else { continue };
        let e = by.entry(acc).or_default();
        if let Some(s) = s {
            e.0 = Some(e.0.unwrap_or(0.0) + s);
        }
        if let Some(w) = w {
            e.1 = Some(e.1.unwrap_or(0.0) + w);
        }
    }
    let name = |id: &str| conn.query_row("SELECT name FROM accounts WHERE id = ?1", params![id], |x| x.get(0)).unwrap_or_else(|_| id.to_string());
    let summed: Vec<UsageSpent> = by.into_iter().filter(|(_, (s, w))| s.is_some() || w.is_some()).map(|(id, (session, weekly))| UsageSpent { account: name(&id), session, weekly }).collect();
    if !summed.is_empty() {
        return Ok(summed);
    }
    Ok(run
        .state
        .usage
        .iter()
        .filter_map(|(id, spent)| {
            let (session, weekly) = spent.delta();
            (session.is_some() || weekly.is_some()).then(|| UsageSpent { account: name(id), session, weekly })
        })
        .collect())
}

pub fn deliveries(conn: &Connection, project_id: &str) -> Result<Vec<Delivery>, String> {
    let by_id = orders_by_id(conn, project_id)?;
    let mut out = Vec::new();
    for r in line::runs(conn, project_id, 200)? {
        let Some(o) = by_id.get(&r.order_id).cloned() else { continue };
        let cited: i64 = conn.query_row("SELECT COUNT(DISTINCT rule_id) FROM factory_decisions WHERE run_id = ?1 AND rule_id IS NOT NULL", params![r.id], |x| x.get(0)).map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare("SELECT DISTINCT rule_id FROM factory_activity WHERE run_id = ?1 AND station = 'dec' AND kind = 'warn' AND rule_id IS NOT NULL").map_err(|e| e.to_string())?;
        let round_rules = stmt.query_map(params![r.id], |x| x.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        let usage = order_usage(conn, &r)?;
        out.push(Delivery {
            usage,
            run_id: r.id,
            order_key: o.key,
            title: o.title,
            status: r.status,
            stage: r.stage,
            mr_url: r.mr_url,
            rounds: r.round,
            round_rules,
            rules_cited: cited,
            cost_usd: r.cost_usd,
            started_at: r.started_at,
            finished_at: r.finished_at,
            error: r.error,
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn factory_live<R: tauri::Runtime>(app: tauri::AppHandle<R>, project_id: String) -> Result<Live, String> {
    crate::factory::with_db(app, move |conn| {
        live(&conn, &project_id)
    })
    .await
}

#[tauri::command]
pub async fn factory_deliveries<R: tauri::Runtime>(app: tauri::AppHandle<R>, project_id: String) -> Result<Vec<Delivery>, String> {
    crate::factory::with_db(app, move |conn| {
        deliveries(&conn, &project_id)
    })
    .await
}
