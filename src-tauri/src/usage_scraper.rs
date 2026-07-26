//! Best-effort real usage stats for a subscription account, scraped from
//! Claude Code's interactive `/usage` slash command.
//!
//! Anthropic's own CLI has no documented headless/programmatic way to query
//! the real 5h/session/weekly quota — `/usage` is a TUI-only built-in, and
//! slash commands don't work under `-p`/`--output-format` (see
//! anthropics/claude-code#837, an open feature request for exactly this).
//! So this drives a real interactive `claude` process in a PTY (same pattern
//! as `login_manager.rs`, the only other place in this codebase that does
//! that), types `/usage`, and parses whatever text comes back. This is
//! inherently fragile — it depends on the exact rendered TUI text, which can
//! change between CLI versions — so every field is `Option<f64>` and a
//! parsing miss degrades to `None`, never a wrong number or a crash. Callers
//! are expected to fall back to Hangar's own token-based estimate for any
//! field that comes back `None`.

use crate::db::DbConnection;
use crate::login_manager::login_env;
use crate::providers::claude;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};

const CACHE_TTL: Duration = Duration::from_secs(300);
const STARTUP_WAIT: Duration = Duration::from_millis(800);
const IDLE_TIMEOUT: Duration = Duration::from_millis(700);
const MAX_TOTAL_WAIT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealUsageStats {
    pub session_pct: Option<f64>,
    pub weekly_pct: Option<f64>,
    pub context_pct: Option<f64>,
    pub raw_text: String,
    pub fetched_at: String,
}

pub struct UsageCache(pub Mutex<HashMap<String, (Instant, RealUsageStats)>>);

impl UsageCache {
    pub fn new() -> Self {
        UsageCache(Mutex::new(HashMap::new()))
    }
}

fn resolve_account_config(conn: &Connection, account_id: &str) -> Result<(String, String), String> {
    conn.query_row(
        "SELECT provider, config_dir_path FROM accounts WHERE id = ?1",
        params![account_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .map_err(|e| format!("account lookup failed: {e}"))
}

/// Strips ANSI CSI escape sequences (`ESC [ ... <final byte>`), the vast
/// majority of what a color/cursor-movement-heavy TUI emits. Not a full
/// terminal emulator — good enough to get plain text back out for regex-ing.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for nc in chars.by_ref() {
                    if nc.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Finds the first `NN` or `NN.N` immediately before a `%` in `line`.
fn extract_percent(line: &str) -> Option<f64> {
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b != b'%' {
            continue;
        }
        let mut start = i;
        while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b'.') {
            start -= 1;
        }
        if start < i {
            if let Ok(v) = line[start..i].parse::<f64>() {
                return Some(v);
            }
        }
    }
    None
}

/// Scans `/usage`'s (ANSI-stripped) text line by line for a percentage
/// attributed to the nearest preceding recognizable keyword — covers both a
/// keyword and its percentage sharing one line ("Context window: 67% used")
/// and a header line followed by its percentage on the next ("Weekly
/// limit\n  18% used"), since real TUI layouts could plausibly do either.
/// Each field is independent — text that never mentions a keyword, or never
/// pairs one with a `%`, just leaves that field `None` rather than failing
/// the whole parse.
fn parse_usage_text(text: &str) -> (Option<f64>, Option<f64>, Option<f64>) {
    let mut session_pct = None;
    let mut weekly_pct = None;
    let mut context_pct = None;
    let mut current: Option<&'static str> = None;
    for raw_line in text.lines() {
        let line = raw_line.to_lowercase();
        if line.contains("week") {
            current = Some("week");
        } else if line.contains("context") {
            current = Some("context");
        } else if line.contains("session") || line.contains("5-hour") || line.contains("5 hour") {
            current = Some("session");
        }
        let Some(pct) = extract_percent(&line) else { continue };
        match current {
            Some("week") => weekly_pct.get_or_insert(pct),
            Some("context") => context_pct.get_or_insert(pct),
            Some("session") => session_pct.get_or_insert(pct),
            _ => continue,
        };
    }
    (session_pct, weekly_pct, context_pct)
}

/// Spawns `claude` interactively in a PTY, types `/usage`, and returns the
/// (ANSI-stripped) accumulated terminal output. Kills the process when done
/// — this is a one-shot query, not a session left running.
fn fetch_usage_text(config_dir: &str) -> Result<String, String> {
    let pty = native_pty_system();
    let pair = pty
        .openpty(PtySize { rows: 30, cols: 120, pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(claude::binary());
    if let Ok(home) = std::env::var("HOME") {
        cmd.cwd(home);
    }
    for (k, v) in login_env(config_dir) {
        cmd.env(k, v);
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    std::thread::sleep(STARTUP_WAIT);
    writer.write_all(b"/usage\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    let mut collected = Vec::new();
    let overall_deadline = Instant::now() + MAX_TOTAL_WAIT;
    let mut last_byte_at = Instant::now();
    loop {
        if Instant::now() >= overall_deadline {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(chunk) => {
                collected.extend_from_slice(&chunk);
                last_byte_at = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if !collected.is_empty() && last_byte_at.elapsed() >= IDLE_TIMEOUT {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let _ = child.kill();
    Ok(strip_ansi(&String::from_utf8_lossy(&collected)))
}

/// At or above this on the 5-hour session window or the weekly window, the
/// account is treated as out of quota. The 5h limit is usually what trips
/// first, so either crossing it is enough.
const EXHAUSTION_PCT: f64 = 95.0;

/// Whether `/usage` says an account has run out of room. `None` percentages
/// (a parse miss) count as *not* exhausted — better to leave an account usable
/// on a scrape we couldn't read than to bench it on a guess.
pub fn is_exhausted(stats: &RealUsageStats) -> bool {
    stats.session_pct.is_some_and(|p| p >= EXHAUSTION_PCT) || stats.weekly_pct.is_some_and(|p| p >= EXHAUSTION_PCT)
}

/// Runs `/usage` against one config dir and parses it — the shared core of the
/// `fetch_real_usage` command and the terminal-rotation poller, so both read
/// the same numbers the same way.
pub fn fetch_usage_stats(config_dir: &str) -> Result<RealUsageStats, String> {
    let raw_text = fetch_usage_text(config_dir)?;
    let (session_pct, weekly_pct, context_pct) = parse_usage_text(&raw_text);
    Ok(RealUsageStats {
        session_pct,
        weekly_pct,
        context_pct,
        raw_text,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    })
}

/// `fetch_usage_stats` behind the shared `UsageCache`.
///
/// Every caller goes through this rather than scraping directly: a check costs
/// a hidden interactive `claude` for up to 6 seconds, and the Accounts screen,
/// the 5-minute terminal poller, and the check fired when a terminal opens can
/// easily all want the same account within a few seconds of each other. One
/// reading inside the TTL serves all of them.
pub fn cached_usage_stats<R: tauri::Runtime>(app: &AppHandle<R>, account_id: &str, config_dir: &str, force: bool) -> Result<RealUsageStats, String> {
    if !force {
        if let Some(cache) = app.try_state::<UsageCache>() {
            if let Ok(map) = cache.0.lock() {
                if let Some((fetched, stats)) = map.get(account_id) {
                    if fetched.elapsed() < CACHE_TTL {
                        return Ok(stats.clone());
                    }
                }
            }
        }
    }

    let stats = fetch_usage_stats(config_dir)?;

    if let Some(cache) = app.try_state::<UsageCache>() {
        if let Ok(mut map) = cache.0.lock() {
            map.insert(account_id.to_string(), (Instant::now(), stats.clone()));
        }
    }

    Ok(stats)
}

#[tauri::command]
pub fn fetch_real_usage(app: AppHandle, db: State<DbConnection>, account_id: String, force: bool) -> Result<RealUsageStats, String> {
    let (provider, config_dir) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        resolve_account_config(&conn, &account_id)?
    };
    if provider != "claude" {
        return Err(format!("usage scraping isn't supported for '{provider}' accounts"));
    }

    cached_usage_stats(&app, &account_id, &config_dir, force)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_csi_sequences_but_keeps_text() {
        let input = "\u{1b}[1;32mSession\u{1b}[0m: \u{1b}[33m42%\u{1b}[0m used";
        assert_eq!(strip_ansi(input), "Session: 42% used");
    }

    #[test]
    fn strip_ansi_leaves_plain_text_untouched() {
        assert_eq!(strip_ansi("no escapes here"), "no escapes here");
    }

    #[test]
    fn extract_percent_finds_integer_and_decimal() {
        assert_eq!(extract_percent("context window: 67% used"), Some(67.0));
        assert_eq!(extract_percent("weekly: 12.5% used"), Some(12.5));
        assert_eq!(extract_percent("no percent here"), None);
    }

    #[test]
    fn parse_usage_text_routes_each_field_by_keyword() {
        let text = "Current session\n  42% used\nWeekly limit\n  18% used, resets Monday\nContext window: 67% used\n";
        let (session, weekly, context) = parse_usage_text(text);
        assert_eq!(session, Some(42.0));
        assert_eq!(weekly, Some(18.0));
        assert_eq!(context, Some(67.0));
    }

    #[test]
    fn parse_usage_text_degrades_to_none_on_unrecognized_text() {
        let (session, weekly, context) = parse_usage_text("hello there\nnothing useful\n");
        assert_eq!(session, None);
        assert_eq!(weekly, None);
        assert_eq!(context, None);
    }

    fn stats(session: Option<f64>, weekly: Option<f64>) -> RealUsageStats {
        RealUsageStats { session_pct: session, weekly_pct: weekly, context_pct: None, raw_text: String::new(), fetched_at: String::new() }
    }

    #[test]
    fn is_exhausted_trips_on_either_window_at_the_threshold() {
        assert!(!is_exhausted(&stats(Some(94.9), Some(10.0))), "just under must not trip");
        assert!(is_exhausted(&stats(Some(95.0), None)), "exactly at the threshold on the 5h window counts");
        assert!(is_exhausted(&stats(Some(10.0), Some(99.0))), "the weekly window alone can trip it");
        assert!(!is_exhausted(&stats(None, None)), "an unreadable scrape must not bench an account");
    }

    #[test]
    fn parse_usage_text_ignores_partial_matches_missing_percent() {
        let text = "session started\nweek in review\ncontext loaded\n";
        let (session, weekly, context) = parse_usage_text(text);
        assert_eq!(session, None);
        assert_eq!(weekly, None);
        assert_eq!(context, None);
    }
}
