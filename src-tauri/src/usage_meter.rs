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

    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    // A shrinking file means it was replaced — a transfer between accounts does
    // exactly that — so start over rather than slicing from a stale offset and
    // landing mid-line.
    let from = if (text.len() as u64) < start { 0 } else { start as usize };
    let fresh = &text[from..];

    if let Ok(mut map) = OFFSETS.lock() {
        map.insert(key, text.len() as u64);
    }
    parse_claude_usage(fresh)
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
    let mut written = 0;
    for turn in turns {
        let cost = estimate_cost_usd(turn);
        let result = conn.execute(
            "INSERT OR IGNORE INTO turn_usage
               (id, session_id, account_id, project_id, block_id, model, provider_message_id,
                input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens, cost_usd, created_at)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            ],
        );
        written += result.unwrap_or(0);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped from a real transcript line, cache fields and all.
    const REAL_LINE: &str = r#"{"type":"assistant","timestamp":"2026-07-21T11:38:20.992Z","message":{"id":"msg_01ABC","model":"claude-sonnet-5","usage":{"input_tokens":2,"cache_creation_input_tokens":23510,"cache_read_input_tokens":28220,"output_tokens":4095}}}"#;

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
