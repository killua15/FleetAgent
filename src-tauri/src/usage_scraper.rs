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
/// How quiet the terminal has to go before we accept that the CLI has finished
/// booting and will hear a slash command. Generous because a real config dir
/// loads MCP servers and plugins first, and typing into that is typing into
/// nothing.
const SETTLE_BEFORE_ASKING: Duration = Duration::from_millis(1200);
const IDLE_TIMEOUT: Duration = Duration::from_millis(900);
/// Six seconds used to be the cap and it wasn't enough for a cold start with
/// MCP servers — the scrape returned the welcome banner and no numbers.
const MAX_TOTAL_WAIT: Duration = Duration::from_secs(25);

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
    // Flat text, no borders, no animations. It exists for screen readers and
    // it's exactly what a parser wants: the decorated TUI redraws the same
    // numbers inside box characters that move between frames.
    cmd.arg("--ax-screen-reader");
    // Starting in `$HOME` made Claude Code ask "is this a folder you trust?"
    // and sit there — the scrape timed out on a security question nobody was
    // there to answer. Answering it for the user would silently grant read,
    // edit and execute over their entire home directory, which is not a
    // decision a background quota check gets to make. So we start in a folder
    // this account *already* trusts, which the CLI records itself.
    cmd.cwd(trusted_workspace(config_dir).unwrap_or_else(|| crate::app_data::home_dir().unwrap_or_else(|| "/".to_string())));
    for (k, v) in login_env(config_dir) {
        cmd.env(k, v);
    }

    let mut child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;

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
    let deadline = Instant::now() + MAX_TOTAL_WAIT;
    let mut last_byte_at = Instant::now();
    let mut asked = false;
    let mut dismissed_onboarding = false;

    loop {
        if Instant::now() >= deadline {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(150)) {
            Ok(chunk) => {
                collected.extend_from_slice(&chunk);
                last_byte_at = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let seen = strip_ansi(&String::from_utf8_lossy(&collected)).to_lowercase();

        // A config dir that has never run interactively opens on the theme
        // picker, and this is where the old scraper died: it typed `/usage`
        // into that menu, got nothing back, and reported "no reading" forever.
        // Every per-account directory Hangar creates starts in exactly that
        // state. Enter accepts whatever is already highlighted.
        if !dismissed_onboarding && seen.contains("enter selection") {
            // It wants a *number*, not Enter — which is why a bare carriage
            // return left the CLI sitting on the menu until the timeout. The
            // option already highlighted is the one to send, so this accepts
            // the default rather than choosing on the user's behalf.
            let answer = selected_option(&seen).unwrap_or(1);
            let _ = writer.write_all(format!("{answer}\r").as_bytes());
            let _ = writer.flush();
            dismissed_onboarding = true;
            last_byte_at = Instant::now();
            continue;
        }

        // Wait for the prompt to settle before typing: a slash command sent
        // while the CLI is still booting (MCP servers, plugins) lands nowhere.
        if !asked && last_byte_at.elapsed() >= SETTLE_BEFORE_ASKING && !collected.is_empty() {
            let _ = writer.write_all(b"/usage\r");
            let _ = writer.flush();
            asked = true;
            last_byte_at = Instant::now();
            continue;
        }

        // Not logged in: the CLI asks how you'd like to sign in. Timing out on
        // that for 25 seconds and reporting "no reading" hides the one thing
        // the user needs to hear.
        if seen.contains("select login method") {
            let _ = child.kill();
            return Err("this account isn't logged in — open it in Accounts and sign in".to_string());
        }

        if asked {
            // The answer is here the moment both windows have a number; no
            // point burning the rest of the timeout on redraws.
            if seen.matches("% used").count() >= 2 {
                break;
            }
            if last_byte_at.elapsed() >= IDLE_TIMEOUT && seen.contains("% used") {
                break;
            }
        }
    }

    let _ = child.kill();
    Ok(strip_ansi(&String::from_utf8_lossy(&collected)))
}

/// A directory this config dir has already accepted the trust prompt for.
///
/// Claude Code keeps that in `<config_dir>/.claude.json`, so this asks the CLI
/// what it already trusts rather than granting anything new.
fn trusted_workspace(config_dir: &str) -> Option<String> {
    let raw = std::fs::read_to_string(std::path::Path::new(config_dir).join(".claude.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    parsed
        .get("projects")?
        .as_object()?
        .iter()
        .find(|(path, settings)| {
            settings.get("hasTrustDialogAccepted").and_then(|v| v.as_bool()).unwrap_or(false) && std::path::Path::new(path).is_dir()
        })
        .map(|(path, _)| path.clone())
}

/// The number of the option a menu already has highlighted, from a line like
/// `2. (selected) Dark mode`.
fn selected_option(text: &str) -> Option<u32> {
    text.lines()
        .find(|line| line.contains("(selected)"))
        .and_then(|line| line.trim().split('.').next()?.trim().parse().ok())
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

#[cfg(test)]
mod real_output_tests {
    use super::*;

    /// Captured verbatim from `claude --ax-screen-reader` v2.1.220. The
    /// duplicated `17% 17%` is real — the TUI repaints the figure — and the
    /// parser has to take the first rather than the last, or a mid-repaint
    /// frame becomes the reading.
    const REAL: &str = "Current session\n17% 17% used\nResets 6:40pm (America/Sao_Paulo)\nCurrent week (all models)\n58% 57% used\nResets Jul 31 at 9pm";

    #[test]
    fn the_real_screen_reader_output_parses() {
        let (session, weekly, _) = parse_usage_text(REAL);
        assert_eq!(session, Some(17.0));
        assert_eq!(weekly, Some(58.0), "`Current week (all models)` has to route to the weekly window");
    }

    /// The theme menu wants a *number*; a bare Enter left the CLI sitting on
    /// it until the timeout, which is why every account read "no reading".
    #[test]
    fn the_highlighted_menu_option_is_the_one_answered() {
        let menu = "1. Auto (match terminal)\n2. (selected) Dark mode\n3. Light mode\nEnter selection [1-7], or Escape to cancel:";
        assert_eq!(selected_option(menu), Some(2));
    }

    /// A menu with nothing marked must not guess: the caller falls back to 1,
    /// and inventing a different number here would pick an option at random.
    #[test]
    fn a_menu_with_no_selection_yields_nothing() {
        assert_eq!(selected_option("1. one\n2. two\nEnter selection [1-2]:"), None);
    }
}

#[cfg(test)]
mod live_tests {
    use super::*;

    /// Runs the real scraper against a real config dir and prints what came
    /// back. `#[ignore]` because it spawns `claude` and needs a logged-in
    /// account — but it's the only way to find out *why* a scrape returns
    /// nothing, which is a question static tests can't answer.
    ///
    /// `cargo test -- --ignored --nocapture live_scrape`
    #[test]
    #[ignore = "spawns a real `claude` and needs a logged-in config dir"]
    fn live_scrape_prints_what_usage_returned() {
        let dir = std::env::var("HANGAR_TEST_CONFIG_DIR").unwrap_or_else(|_| format!("{}/.claude", crate::app_data::home_dir().unwrap()));
        let text = fetch_usage_text(&dir).expect("the scrape itself must not error");
        println!("--- {} bytes captured from {dir} ---", text.len());
        println!("{text}");
        println!("--- parsed: {:?} ---", parse_usage_text(&text));
    }
}
