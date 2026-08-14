//! What each turn actually cost, read from what the CLIs already write down.
//!
//! **The key fact is that we don't have to parse the terminal.** Claude Code
//! records `usage` on every assistant message in its transcript
//! (`<config_dir>/projects/<slug>/<uuid>.jsonl`) — the same file the handoff
//! feature already reads — and Codex writes `token_count` events into
//! `<CODEX_HOME>/sessions/…`. Scraping the PTY for numbers the tool publishes
//! to disk would be inventing a fragile problem.
//!
//! Read when an agent goes idle rather than on a timer: that's the moment the
//! turn is over and the file is complete.

use crate::db::DbConnection;
use rusqlite::params;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
use uuid::Uuid;

/// One assistant turn's token counts.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnCost {
    /// The CLI's own message id — what makes re-reading the file idempotent.
    pub message_id: String,
    pub model: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub at: Option<String>,
}

/// How far into each transcript we've already read, by file path.
///
/// A `.jsonl` only grows, so the offset turns "re-read a 5 MB file on every
/// idle event" into "read the new bytes". It's in memory rather than in the
/// database on purpose: losing it costs one re-read, which the unique index on
/// `provider_message_id` makes harmless, and that's cheaper than a table whose
/// rows can go stale against a file that was deleted.
static OFFSETS: Mutex<BTreeMap<String, u64>> = Mutex::new(BTreeMap::new());

/// Pulls whatever is new out of a Claude transcript.
pub fn read_claude_transcript(config_dir: &str, session_uuid: &str) -> Vec<TurnCost> {
    let Some(path) = crate::handoff::find_transcript(config_dir, session_uuid) else { return Vec::new() };
    let key = path.to_string_lossy().to_string();
    let start = OFFSETS.lock().ok().and_then(|map| map.get(&key).copied()).unwrap_or(0);

    // Seek to the offset and read only what's new. This used to be
    // `read_to_string` of the whole file followed by a slice, which meant
    // allocating the entire transcript to look at its last few kilobytes — and
    // these grow: 50 MB for one session on the machine this was written on,
    // 326 MB across all of them. On a laptop already paging, doing that every
    // time an agent finishes a turn is how a background job turns into a window
    // that stops responding.
    let Ok(mut file) = std::fs::File::open(&path) else { return Vec::new() };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    // A shrinking file means it was replaced — a transfer between accounts does
    // exactly that — so start over rather than reading from a stale offset and
    // landing mid-line.
    let from = if len < start { 0 } else { start };
    if from > 0 && file.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    let mut fresh = String::new();
    if file.read_to_string(&mut fresh).is_err() {
        return Vec::new();
    }

    if let Ok(mut map) = OFFSETS.lock() {
        map.insert(key, len);
    }
    parse_claude_usage(&fresh)
}

/// The pure half, so the shape of a real transcript can be tested without one.
pub fn parse_claude_usage(text: &str) -> Vec<TurnCost> {
    let mut turns = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        let Some(message) = value.get("message") else { continue };
        let Some(usage) = message.get("usage") else { continue };
        // Without an id there's no way to tell this turn from the same turn
        // read again, and counting it would inflate the week on every restart.
        let Some(message_id) = message.get("id").and_then(|id| id.as_str()) else { continue };

        turns.push(TurnCost {
            message_id: message_id.to_string(),
            model: message.get("model").and_then(|m| m.as_str()).map(str::to_string),
            input_tokens: number(usage, "input_tokens"),
            output_tokens: number(usage, "output_tokens"),
            cache_read_tokens: number(usage, "cache_read_input_tokens"),
            cache_creation_tokens: number(usage, "cache_creation_input_tokens"),
            at: value.get("timestamp").and_then(|t| t.as_str()).map(str::to_string),
        });
    }
    turns
}

/// Codex reports a *running total* per turn, not a delta, plus the rate-limit
/// percentage straight from the API — which is the quota reading Codex has no
/// `/usage` command to give.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexTurn {
    pub input_tokens: i64,
    pub cached_input_tokens: i64,
    pub output_tokens: i64,
    pub used_percent: Option<f64>,
    pub at: Option<String>,
}

/// Codex's own quota reading, taken from the last turn it wrote.
///
/// It maps to the same two windows Claude reports: `primary` is the short
/// rolling one, `secondary` the long one. No PTY and no `/usage` — Codex has no
/// such command, and it doesn't need one, because it records the API's
/// rate-limit headers on every turn.
pub fn codex_quota(codex_home: &str) -> Option<(Option<f64>, Option<f64>)> {
    let newest = newest_rollout(std::path::Path::new(codex_home))?;
    let text = std::fs::read_to_string(newest).ok()?;
    // Last one wins: these are cumulative readings, and the most recent is the
    // only one that describes now.
    let mut found = None;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let Some(limits) = value.get("payload").and_then(|p| p.get("rate_limits")) else { continue };
        let pct = |key: &str| limits.get(key).and_then(|w| w.get("used_percent")).and_then(|v| v.as_f64());
        found = Some((pct("primary"), pct("secondary")));
    }
    found
}

/// The id Codex gave the session it ran in `cwd`, if there is one.
///
/// Codex has no flag to name a session at start — so unlike Claude, Hangar
/// can't choose the id and has to read back the one the CLI assigned. It writes
/// it into the rollout's `session_meta`, along with the working directory,
/// which is what makes the match trustworthy: with six terminals open under one
/// account, "the newest session" belongs to whichever agent last spoke, not to
/// the one being asked about.
pub fn assigned_session_id(codex_home: &str, cwd: &str) -> Option<String> {
    let mut best: Option<(std::time::SystemTime, String)> = None;
    for path in rollouts(std::path::Path::new(codex_home)) {
        let Ok(modified) = std::fs::metadata(&path).and_then(|m| m.modified()) else { continue };
        if best.as_ref().is_some_and(|(seen, _)| modified <= *seen) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        // `session_meta` is the first line, so this stops after it rather than
        // parsing a transcript that can be megabytes.
        let Some(meta) = text.lines().next().and_then(|line| serde_json::from_str::<Value>(line).ok()) else { continue };
        let payload = meta.get("payload").unwrap_or(&meta);
        if payload.get("cwd").and_then(|c| c.as_str()) != Some(cwd) {
            continue;
        }
        let Some(id) = payload.get("id").or_else(|| payload.get("session_id")).and_then(|v| v.as_str()) else { continue };
        best = Some((modified, id.to_string()));
    }
    best.map(|(_, id)| id)
}

/// Whether Codex still has this session on disk — the same question
/// `handoff::transcript_exists` answers for Claude, against a different layout.
/// The id is in the filename, so this costs a directory walk rather than a read.
pub fn rollout_exists(codex_home: &str, session_id: &str) -> bool {
    rollouts(std::path::Path::new(codex_home))
        .iter()
        .any(|path| path.file_name().is_some_and(|name| name.to_string_lossy().contains(session_id)))
}

/// Codex files sessions under `sessions/YYYY/MM/DD/`, so the newest one is
/// found by walking rather than by listing a single directory.
fn newest_rollout(codex_home: &std::path::Path) -> Option<std::path::PathBuf> {
    rollouts(codex_home)
        .into_iter()
        .filter_map(|path| std::fs::metadata(&path).and_then(|m| m.modified()).ok().map(|at| (at, path)))
        .max_by_key(|(at, _)| *at)
        .map(|(_, path)| path)
}

/// Every rollout file under a Codex home, at any depth.
fn rollouts(codex_home: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![codex_home.join("sessions")];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl") {
                found.push(path);
            }
        }
    }
    found
}

pub fn parse_codex_usage(text: &str) -> Vec<CodexTurn> {
    let mut turns = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else { continue };
        let Some(payload) = value.get("payload") else { continue };
        if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
            continue;
        }
        // `last_token_usage` is this turn; `total_token_usage` is the session so
        // far. Summing the totals would count every earlier turn again.
        let Some(last) = payload.get("info").and_then(|i| i.get("last_token_usage")) else { continue };
        turns.push(CodexTurn {
            input_tokens: number(last, "input_tokens"),
            cached_input_tokens: number(last, "cached_input_tokens"),
            output_tokens: number(last, "output_tokens"),
            used_percent: payload.get("rate_limits").and_then(|r| r.get("primary")).and_then(|p| p.get("used_percent")).and_then(|v| v.as_f64()),
            at: value.get("timestamp").and_then(|t| t.as_str()).map(str::to_string),
        });
    }
    turns
}

fn number(value: &Value, key: &str) -> i64 {
    value.get(key).and_then(|v| v.as_i64()).unwrap_or(0)
}

/// Writes what a terminal spent, ignoring turns already recorded.
///
/// `INSERT OR IGNORE` against the unique index on `provider_message_id` is what
/// makes this safe to call as often as we like — the alternative, checking
/// first, races with itself the moment two terminals go idle together.
pub fn record_turns(db: &DbConnection, account_id: &str, project_id: Option<&str>, session_id: Option<&str>, turns: &[TurnCost]) -> usize {
    let Ok(conn) = db.0.lock() else { return 0 };
    // Whether a graph was available *now*, when the turn ran. Recorded rather
    // than derived later, so building one next week doesn't rewrite this
    // week's history and flatter the comparison (Phase 3).
    let with_graph = project_id.is_some_and(|id| crate::commands::graph::graph_available_for_project(&conn, id));

    // One transaction for the batch, not one per row. Every statement outside a
    // transaction is its own commit, and a commit is an fsync — measured at
    // 1.7s for the ~5,900 rows a first read of a long transcript produces, all
    // of it holding the one database lock the whole app shares. Inside a
    // transaction the same batch is tens of milliseconds.
    //
    // `unchecked_transaction` because the connection is behind a shared lock
    // and we only have `&Connection`. On error we carry on without one: a slow
    // write beats no write.
    let tx = conn.unchecked_transaction().ok();

    let mut written = 0;
    for turn in turns {
        let cost = estimate_cost_usd(turn);
        let result = conn.execute(
            "INSERT OR IGNORE INTO turn_usage
               (id, session_id, account_id, project_id, block_id, model, provider_message_id,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, cost_usd, created_at, with_graph)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                Uuid::new_v4().to_string(),
                session_id,
                account_id,
                project_id,
                turn.model,
                turn.message_id,
                turn.input_tokens,
                turn.output_tokens,
                turn.cache_read_tokens,
                turn.cache_creation_tokens,
                cost,
                turn.at.clone().unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
                with_graph as i64,
            ],
        );
        written += result.unwrap_or(0);
    }

    if let Some(tx) = tx {
        if let Err(e) = tx.commit() {
            // These turns are gone until the transcript is read from the start
            // again, because `read_claude_transcript` has already advanced its
            // offset past them. Loud for that reason — it is silent data loss,
            // not a retry — though it takes a full disk or a corrupt database
            // to get here.
            log::error!("usage: could not commit {written} recorded turn(s), they will not be counted: {e}");
            return 0;
        }
    }
    written
}

/// Per-million-token prices, in US dollars: (input, output, cache write, cache read).
///
/// Four numbers, not two. Cache reads cost a tenth of input, and a real turn
/// here is 28k of them against 2 actual input tokens — so charging them at the
/// input rate is a 10× error on that component and ~40% on the turn as a whole.
/// Wrong by half is worse than absent: nobody double-checks a number.
///
/// Matched by prefix so a dated model id (`claude-sonnet-5-20260514`) resolves
/// to its family, and an unknown model costs nothing rather than guessing: a
/// missing number reads as "unknown", a wrong one reads as fact.
const PRICES: &[(&str, f64, f64, f64, f64)] = &[
    ("claude-opus", 15.0, 75.0, 18.75, 1.50),
    ("claude-sonnet", 3.0, 15.0, 3.75, 0.30),
    ("claude-haiku", 0.80, 4.0, 1.0, 0.08),
    ("gpt-5", 1.25, 10.0, 1.25, 0.125),
    ("o4", 1.10, 4.40, 1.10, 0.275),
];

pub fn estimate_cost_usd(turn: &TurnCost) -> Option<f64> {
    let model = turn.model.as_deref()?.to_lowercase();
    let (_, input, output, cache_write, cache_read) = PRICES.iter().find(|(prefix, ..)| model.starts_with(prefix))?;
    Some(
        (turn.input_tokens as f64 * input
            + turn.output_tokens as f64 * output
            + turn.cache_creation_tokens as f64 * cache_write
            + turn.cache_read_tokens as f64 * cache_read)
            / 1_000_000.0,
    )
}

/// What one dollar of spend costs you, in percentage points of your weekly
/// quota. **Measured, not assumed.**
///
/// Nobody publishes the formula that turns tokens into a subscription's quota,
/// and it differs by plan. But we have both halves already: `/usage` gives the
/// real percentage at known times, and `turn_usage` gives what was spent
/// between them. The ratio is this account's own exchange rate.
///
/// Calibrated against the dollar equivalent rather than raw tokens on purpose:
/// that folds four token prices into one number, so a week of cache-heavy
/// turns and a week of output-heavy ones don't produce two different answers.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Calibration {
    pub pct_per_dollar: f64,
    /// How many (sample, sample) spans it rests on, so the UI can say "from
    /// two readings" instead of implying certainty.
    pub spans: usize,
}

/// Percentage points and dollars for one span between two readings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub pct_delta: f64,
    pub dollars: f64,
}

/// The pure rule, so it's testable without a database or five hours of waiting.
///
/// Spans that crossed a window reset must never reach this: a drop from 90% to
/// 4% is a new window, not 86 points of refund, and one of those would poison
/// the rate into uselessness. `samples_since_reset` is what keeps them out.
pub fn calibrate(spans: &[Span]) -> Option<Calibration> {
    // Only spans where both halves moved. A span with spend but no percentage
    // change is real (rounding), and one with percentage change but no
    // recorded spend means work happened outside Hangar — including it would
    // blame our turns for someone else's burn.
    let usable: Vec<&Span> = spans.iter().filter(|s| s.dollars > 0.01 && s.pct_delta > 0.0).collect();
    if usable.is_empty() {
        return None;
    }
    // Totals rather than an average of ratios: a tiny span with a rounding-sized
    // percentage move would otherwise carry the same weight as a full day.
    let pct: f64 = usable.iter().map(|s| s.pct_delta).sum();
    let dollars: f64 = usable.iter().map(|s| s.dollars).sum();
    Some(Calibration { pct_per_dollar: pct / dollars, spans: usable.len() })
}

/// One account's spending, as the modal shows it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSpend {
    pub account_id: String,
    pub account_name: String,
    pub provider: String,
    /// `sub` or `key`. This decides which number leads: a subscription burns a
    /// **percentage** and the money is a market equivalent nobody charges you;
    /// a key-billed account burns **money** and has no percentage at all.
    pub billing: String,
    pub session_pct: Option<f64>,
    pub weekly_pct: Option<f64>,
    /// Hours until the weekly window runs out at the current rate.
    pub hours_left: Option<f64>,
    pub session: TokenTotals,
    pub week: TokenTotals,
    /// This account's measured %-per-dollar, when there's enough evidence.
    pub calibration: Option<Calibration>,
    /// The week's spend expressed in quota points, via `calibration` — the
    /// number that means something on a subscription.
    pub week_pct_equivalent: Option<f64>,
    pub by_project: Vec<ProjectSpend>,
    pub by_model: Vec<ModelSpend>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cost_usd: f64,
    pub turns: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectSpend {
    pub project_name: String,
    pub cost_usd: f64,
    pub turns: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpend {
    pub model: String,
    pub cost_usd: f64,
    pub turns: i64,
}

fn totals_since(conn: &rusqlite::Connection, account_id: &str, since: &str) -> TokenTotals {
    conn.query_row(
        "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_creation_tokens),0),
                COALESCE(SUM(cost_usd),0.0), COUNT(*)
         FROM turn_usage WHERE account_id = ?1 AND created_at >= ?2",
        params![account_id, since],
        |row| {
            Ok(TokenTotals {
                input_tokens: row.get(0)?,
                output_tokens: row.get(1)?,
                cache_read_tokens: row.get(2)?,
                cache_creation_tokens: row.get(3)?,
                cost_usd: row.get(4)?,
                turns: row.get(5)?,
            })
        },
    )
    .unwrap_or_default()
}

/// Pairs each `/usage` reading with what was spent since the previous one.
///
/// Only within one window: `samples_since_reset` drops everything before the
/// last reset, so a span that straddles the boundary can't get in.
fn spans_for(conn: &rusqlite::Connection, account_id: &str) -> Vec<Span> {
    let Ok(samples) = crate::quota::weekly_samples(conn, account_id) else { return Vec::new() };
    let samples = crate::quota::samples_since_reset(&samples);
    samples
        .windows(2)
        .filter_map(|pair| {
            let dollars: f64 = conn
                .query_row(
                    "SELECT COALESCE(SUM(cost_usd),0.0) FROM turn_usage
                     WHERE account_id = ?1 AND created_at > ?2 AND created_at <= ?3",
                    params![account_id, pair[0].sampled_at.to_rfc3339(), pair[1].sampled_at.to_rfc3339()],
                    |row| row.get(0),
                )
                .unwrap_or(0.0);
            Some(Span { pct_delta: pair[1].pct - pair[0].pct, dollars })
        })
        .collect()
}

/// Every account's spending, for the usage modal.
#[tauri::command]
pub fn list_account_spend(db: tauri::State<DbConnection>) -> Result<Vec<AccountSpend>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    account_spend(&conn)
}

/// The command's body, over a plain connection so it's testable without a Tauri
/// app — same split as `quota::account_quotas`.
pub(crate) fn account_spend(conn: &rusqlite::Connection) -> Result<Vec<AccountSpend>, String> {
    let quotas = crate::quota::account_quotas(conn)?;

    let mut stmt = conn
        .prepare("SELECT id, name, provider, billing FROM accounts ORDER BY name")
        .map_err(|e| e.to_string())?;
    let accounts: Vec<(String, String, String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    drop(stmt);

    // The 5-hour window, not "today": it's the one that actually gates you, and
    // it's what `/usage` reports next to the weekly figure.
    let session_since = (chrono::Utc::now() - chrono::Duration::hours(5)).to_rfc3339();
    let week_since = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();

    let mut out = Vec::new();
    for (account_id, account_name, provider, billing) in accounts {
        let quota = quotas.iter().find(|q| q.account_id == account_id);
        let week = totals_since(conn, &account_id, &week_since);
        let calibration = calibrate(&spans_for(conn, &account_id));

        out.push(AccountSpend {
            session_pct: quota.and_then(|q| q.session.as_ref()).map(|f| f.pct),
            weekly_pct: quota.and_then(|q| q.weekly.as_ref()).map(|f| f.pct),
            hours_left: quota.and_then(|q| q.weekly.as_ref()).and_then(|f| f.hours_left),
            session: totals_since(conn, &account_id, &session_since),
            // Only for subscriptions: on a key-billed account the money is the
            // real number and a quota percentage doesn't exist to convert to.
            week_pct_equivalent: calibration.as_ref().filter(|_| billing == "sub").map(|c| c.pct_per_dollar * week.cost_usd),
            by_project: project_breakdown(conn, &account_id, &week_since),
            by_model: model_breakdown(conn, &account_id, &week_since),
            week,
            calibration,
            account_id,
            account_name,
            provider,
            billing,
        });
    }
    Ok(out)
}

/// What one project spent on one account.
///
/// The pair is the unit, not either half: "this project cost $12" says nothing
/// about which subscription is about to run out, and "this account is at 80%"
/// says nothing about what to stop doing.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAccountSpend {
    pub project_name: String,
    pub account_id: String,
    pub account_name: String,
    pub provider: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub turns: i64,
    pub cost_usd: f64,
}

/// Every project × account pair with spend in the last 7 days.
#[tauri::command]
pub fn list_project_spend(db: tauri::State<DbConnection>) -> Result<Vec<ProjectAccountSpend>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let since = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(p.name, 'no project'), tu.account_id, COALESCE(a.name, '?'), COALESCE(a.provider, '?'),
                    COALESCE(SUM(tu.input_tokens),0), COALESCE(SUM(tu.output_tokens),0), COALESCE(SUM(tu.cache_read_tokens),0),
                    COUNT(*), COALESCE(SUM(tu.cost_usd),0.0)
             FROM turn_usage tu
             LEFT JOIN projects p ON p.id = tu.project_id
             LEFT JOIN accounts a ON a.id = tu.account_id
             WHERE tu.created_at >= ?1
             GROUP BY tu.project_id, tu.account_id
             ORDER BY 9 DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![since], |row| {
            Ok(ProjectAccountSpend {
                project_name: row.get(0)?,
                account_id: row.get(1)?,
                account_name: row.get(2)?,
                provider: row.get(3)?,
                input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                cache_read_tokens: row.get(6)?,
                turns: row.get(7)?,
                cost_usd: row.get(8)?,
            })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

fn project_breakdown(conn: &rusqlite::Connection, account_id: &str, since: &str) -> Vec<ProjectSpend> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT COALESCE(p.name, 'no project'), COALESCE(SUM(tu.cost_usd),0.0), COUNT(*)
         FROM turn_usage tu LEFT JOIN projects p ON p.id = tu.project_id
         WHERE tu.account_id = ?1 AND tu.created_at >= ?2
         GROUP BY tu.project_id ORDER BY 2 DESC LIMIT 6",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![account_id, since], |row| Ok(ProjectSpend { project_name: row.get(0)?, cost_usd: row.get(1)?, turns: row.get(2)? }))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

fn model_breakdown(conn: &rusqlite::Connection, account_id: &str, since: &str) -> Vec<ModelSpend> {
    let Ok(mut stmt) = conn.prepare(
        "SELECT COALESCE(model, 'unknown'), COALESCE(SUM(cost_usd),0.0), COUNT(*)
         FROM turn_usage WHERE account_id = ?1 AND created_at >= ?2
         GROUP BY model ORDER BY 2 DESC LIMIT 6",
    ) else {
        return Vec::new();
    };
    stmt.query_map(params![account_id, since], |row| Ok(ModelSpend { model: row.get(0)?, cost_usd: row.get(1)?, turns: row.get(2)? }))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped from a real transcript line, cache fields and all.
    const REAL_LINE: &str = r#"{"type":"assistant","timestamp":"2026-07-21T11:38:20.992Z","message":{"id":"msg_01ABC","model":"claude-sonnet-5","usage":{"input_tokens":2,"cache_creation_input_tokens":23510,"cache_read_input_tokens":28220,"output_tokens":4095}}}"#;

    /// The reader seeks to where it stopped instead of loading the file.
    ///
    /// It used to `read_to_string` the whole transcript and slice off the tail —
    /// 50 MB allocated to look at two lines, every time an agent finished a
    /// turn, on transcripts that only grow. This pins the behaviour that
    /// replaced it: same turns out, only the new ones on the second pass, and a
    /// truncated file starts over instead of reading from a stale offset and
    /// landing mid-line.
    #[test]
    fn the_transcript_is_tailed_from_where_it_stopped() {
        use std::io::Write;

        let dir = std::env::temp_dir().join(format!("hangar-tail-{}", Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("projects/slug")).unwrap();
        let session = Uuid::new_v4().to_string();
        let path = dir.join("projects/slug").join(format!("{session}.jsonl"));
        let config_dir = dir.to_string_lossy().to_string();

        std::fs::write(&path, format!("{REAL_LINE}\n")).unwrap();
        let first = read_claude_transcript(&config_dir, &session);
        assert_eq!(first.len(), 1, "the first read returns what is there");

        // Nothing appended: nothing new to report.
        assert!(read_claude_transcript(&config_dir, &session).is_empty(), "re-reading an unchanged file must return nothing");

        // Appended: only the new line comes back, not the whole file again.
        let second = REAL_LINE.replace("msg_01ABC", "msg_01DEF");
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{second}").unwrap();
        drop(file);
        let third = read_claude_transcript(&config_dir, &session);
        assert_eq!(third.len(), 1, "only the appended turn");
        assert_eq!(third[0].message_id, "msg_01DEF");

        // Replaced by something shorter — what an account transfer does. The
        // stored offset is past the end now, so it has to start over.
        std::fs::write(&path, format!("{REAL_LINE}\n")).unwrap();
        let fourth = read_claude_transcript(&config_dir, &session);
        assert_eq!(fourth.len(), 1, "a shrunken file is read from the beginning");
        assert_eq!(fourth[0].message_id, "msg_01ABC");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_turn_is_read_with_its_four_token_kinds() {
        let turns = parse_claude_usage(REAL_LINE);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].message_id, "msg_01ABC");
        assert_eq!(turns[0].input_tokens, 2);
        assert_eq!(turns[0].output_tokens, 4095);
        assert_eq!(turns[0].cache_read_tokens, 28220);
        assert_eq!(turns[0].cache_creation_tokens, 23510);
    }

    /// Cache reads cost a tenth of input and this turn has 28k of them against
    /// 2 real input tokens. Folding them into `input_tokens` — the obvious
    /// simplification — overcharges that component 10× and the turn ~40%.
    #[test]
    fn cache_tokens_are_priced_apart_from_input() {
        let turn = parse_claude_usage(REAL_LINE).remove(0);
        let real = estimate_cost_usd(&turn).unwrap();

        let naive = TurnCost { input_tokens: 2 + 23510 + 28220, cache_read_tokens: 0, cache_creation_tokens: 0, ..turn };
        let wrong = estimate_cost_usd(&naive).unwrap();
        assert!(wrong > real * 1.3, "naive={wrong} vs real={real} — the whole point of keeping four counters");
    }

    /// Lines without usage — user messages, tool results, summaries — are most
    /// of the file.
    #[test]
    fn lines_without_usage_are_skipped() {
        assert!(parse_claude_usage(r#"{"type":"user","message":{"role":"user","content":"hola"}}"#).is_empty());
        assert!(parse_claude_usage("not json at all").is_empty());
    }

    /// No id, no way to tell a re-read from a new turn — so it's dropped rather
    /// than counted twice on the next restart.
    #[test]
    fn a_turn_without_a_message_id_is_not_counted() {
        let line = r#"{"type":"assistant","message":{"model":"claude-sonnet-5","usage":{"input_tokens":10,"output_tokens":20}}}"#;
        assert!(parse_claude_usage(line).is_empty());
    }

    /// An unknown model has no price. Nothing is a truthful answer; a number
    /// invented from the nearest family is not.
    #[test]
    fn an_unknown_model_has_no_cost_rather_than_a_guessed_one() {
        let turn = TurnCost {
            message_id: "m".into(),
            model: Some("some-new-model".into()),
            input_tokens: 1000,
            output_tokens: 1000,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            at: None,
        };
        assert_eq!(estimate_cost_usd(&turn), None);
    }

    /// A dated model id has to resolve to its family, or every real session
    /// costs nothing.
    #[test]
    fn a_dated_model_id_still_finds_its_price() {
        let turn = TurnCost {
            message_id: "m".into(),
            model: Some("claude-opus-5-20260514".into()),
            input_tokens: 1_000_000,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
            at: None,
        };
        assert_eq!(estimate_cost_usd(&turn), Some(15.0));
    }

    /// Codex names its own sessions, so the id has to be read back — and the
    /// match is by working directory, not by "newest". With several terminals
    /// open under one account the newest rollout belongs to whichever agent
    /// last spoke, which would resume the wrong conversation in the wrong repo.
    #[test]
    fn a_codex_session_is_matched_by_its_working_directory() {
        let home = std::env::temp_dir().join(format!("hangar-test-codex-{}", Uuid::new_v4()));
        let day = home.join("sessions/2026/07/28");
        std::fs::create_dir_all(&day).unwrap();

        let meta = |id: &str, cwd: &str| {
            format!(r#"{{"type":"session_meta","payload":{{"id":"{id}","session_id":"{id}","cwd":"{cwd}"}}}}"#)
        };
        std::fs::write(day.join("rollout-a-1111.jsonl"), meta("1111", "/repos/alpha")).unwrap();
        std::fs::write(day.join("rollout-b-2222.jsonl"), meta("2222", "/repos/beta")).unwrap();

        let home = home.to_string_lossy().to_string();
        assert_eq!(assigned_session_id(&home, "/repos/alpha").as_deref(), Some("1111"));
        assert_eq!(assigned_session_id(&home, "/repos/beta").as_deref(), Some("2222"));
        assert_eq!(assigned_session_id(&home, "/repos/never-used"), None, "no session for that directory is not the newest one");

        // And restore has to be able to ask whether the id still exists.
        assert!(rollout_exists(&home, "2222"));
        assert!(!rollout_exists(&home, "9999"));

        let _ = std::fs::remove_dir_all(&home);
    }

    /// The exchange rate is measured from the user's own data, so a plan
    /// nobody published a formula for still gets an answer.
    #[test]
    fn the_percent_per_dollar_rate_is_measured_from_real_spans() {
        let rate = calibrate(&[Span { pct_delta: 4.0, dollars: 2.0 }, Span { pct_delta: 6.0, dollars: 3.0 }]).unwrap();
        assert_eq!(rate.pct_per_dollar, 2.0);
        assert_eq!(rate.spans, 2);
    }

    /// Weighted by totals, not averaged across spans: otherwise a two-minute
    /// span whose percentage moved by a rounding step counts as much as a
    /// whole day of work, and the rate swings wildly between refreshes.
    #[test]
    fn a_tiny_span_does_not_weigh_as_much_as_a_long_one() {
        let big = Span { pct_delta: 20.0, dollars: 10.0 }; // 2 %/$
        let tiny = Span { pct_delta: 0.1, dollars: 0.02 }; // 5 %/$, noise
        let rate = calibrate(&[big, tiny]).unwrap();
        assert!(rate.pct_per_dollar < 2.1, "totals must dominate: got {}", rate.pct_per_dollar);
    }

    /// Percentage moving with nothing recorded means work happened outside
    /// Hangar — another terminal, another machine. Counting it would blame our
    /// turns for someone else's burn and inflate every future estimate.
    #[test]
    fn burn_with_no_recorded_spend_is_not_used_to_calibrate() {
        assert_eq!(calibrate(&[Span { pct_delta: 30.0, dollars: 0.0 }]), None);
    }

    /// No evidence yet is a real state, and it has to read as "calibrating"
    /// rather than as a number.
    #[test]
    fn nothing_to_learn_from_yields_no_rate() {
        assert_eq!(calibrate(&[]), None);
        assert_eq!(calibrate(&[Span { pct_delta: 0.0, dollars: 5.0 }]), None);
    }

    fn seeded_db() -> DbConnection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        // Mirrors `db::init`: schema then migrations. The unique index that
        // makes recording idempotent is created by migration 12, so a test DB
        // built from the schema alone would silently allow the duplicate this
        // very test exists to catch.
        crate::db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Personal','personal','','','acc1','healthy','now','now')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Sim','/tmp/sim','now')", []).unwrap();
        DbConnection(std::sync::Mutex::new(conn))
    }

    /// The whole point of the unique message id, end to end: reading the same
    /// transcript twice — which happens on every restart, since the byte offset
    /// lives in memory — must not charge the week twice.
    #[test]
    fn reading_the_same_transcript_twice_does_not_double_the_bill() {
        let db = seeded_db();
        let turns = parse_claude_usage(REAL_LINE);

        assert_eq!(record_turns(&db, "acc1", Some("proj1"), None, &turns), 1);
        assert_eq!(record_turns(&db, "acc1", Some("proj1"), None, &turns), 0, "the second read is already recorded");

        let (rows, output): (i64, i64) = {
            let conn = db.0.lock().unwrap();
            conn.query_row("SELECT COUNT(*), COALESCE(SUM(output_tokens),0) FROM turn_usage", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap()
        };
        assert_eq!(rows, 1);
        assert_eq!(output, 4095, "one turn's worth, not two");
    }

    /// A terminal opened on a project has no session row. That case is most of
    /// the real work, and the table it writes to was built without it — a
    /// `NOT NULL` session id would have rejected every one of these.
    #[test]
    fn a_terminal_with_no_session_still_records_what_it_spent() {
        let db = seeded_db();
        // `at: None` means "now": the sample line is a real one from a week
        // ago, and the week window would legitimately exclude it.
        let turns: Vec<TurnCost> = parse_claude_usage(REAL_LINE).into_iter().map(|t| TurnCost { at: None, ..t }).collect();
        assert_eq!(record_turns(&db, "acc1", Some("proj1"), None, &turns), 1);

        let conn = db.0.lock().unwrap();
        let spend = account_spend(&conn).unwrap();
        let personal = spend.iter().find(|a| a.account_id == "acc1").unwrap();
        assert_eq!(personal.week.turns, 1);
        assert_eq!(personal.week.output_tokens, 4095);
        assert_eq!(personal.by_project.first().map(|p| p.project_name.as_str()), Some("Sim"));
        assert_eq!(personal.by_model.first().map(|m| m.model.as_str()), Some("claude-sonnet-5"));
    }

    /// Codex reports a running total *and* the turn's own usage. Reading the
    /// total would re-count every earlier turn in the session.
    #[test]
    fn codex_reports_the_turn_not_the_running_total() {
        let line = r#"{"timestamp":"2026-07-27T00:17:03.149Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":99999,"output_tokens":8888},"last_token_usage":{"input_tokens":14468,"cached_input_tokens":11008,"output_tokens":17}},"rate_limits":{"primary":{"used_percent":42.5}}}}"#;
        let turns = parse_codex_usage(line);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0].input_tokens, 14468, "the turn, not the session total");
        assert_eq!(turns[0].output_tokens, 17);
        assert_eq!(turns[0].used_percent, Some(42.5), "Codex publishes the quota Codex has no command to ask for");
    }
}
