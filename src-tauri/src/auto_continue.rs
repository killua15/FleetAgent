//! Continuing a terminal's task on its own once its usage limit resets — only
//! in the terminals the user marked.
//!
//! **Most of this is Claude Code's, not ours.** Its `autoContinueAtUsageLimit`
//! setting (on by default) already waits for a claude.ai limit to reset and
//! carries on. What it can't do is tell terminals apart: on by default means
//! *every* terminal resumes, which is exactly what the user did not ask for.
//! So Hangar decides it per terminal, through a `--settings` file each CLI is
//! started with (`write_terminal_settings`).
//!
//! Hangar only acts itself in the two cases where Claude Code says, on screen,
//! that it will not resume on its own:
//!
//! 1. the reset is more than 24 hours out — the weekly window;
//! 2. the CLI exited or was relaunched during the wait — which is what a
//!    restart of Hangar does to every terminal it restores.
//!
//! Both messages end with the same advice, "send a prompt after the reset", so
//! that is what Hangar does: types one, a couple of minutes after the exact
//! reset time the CLI itself reported to its status line (`rate_limits`).

use chrono::{DateTime, TimeZone, Utc};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// What Hangar types once the limit has reset. A prompt, not a keystroke: after
/// either give-up message the CLI is back at its input, waiting for one.
pub const CONTINUE_PROMPT: &str = "Continue where you left off.";

/// Claude Code's own words when it declines to wait. Both give-up messages
/// share this phrase ("…so the task will not resume on its own…"), and nothing
/// else it prints does.
const NATIVE_GAVE_UP: &str = "will not resume on its own";

/// After the reset, not on it: the window's edge is where a request is most
/// likely to be refused again, and a spread keeps several armed terminals from
/// all firing in the same second.
const JITTER_SECS: std::ops::RangeInclusive<i64> = 120..=180;

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct Window {
    pub used_percentage: f64,
    /// Unix epoch seconds.
    pub resets_at: i64,
}

/// The two subscription windows from Claude Code's status line input. Either
/// can be absent: the CLI only includes one while the API reports it and its
/// reset hasn't passed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct RateLimits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
}

/// `rate_limits` out of one status line payload, or `None` when it has none —
/// an API-key account, or a CLI that hasn't had its first response yet.
pub fn parse_rate_limits(status_json: &str) -> Option<RateLimits> {
    #[derive(Deserialize)]
    struct Payload {
        rate_limits: Option<RateLimits>,
    }
    let limits = serde_json::from_str::<Payload>(status_json).ok()?.rate_limits?;
    (limits.five_hour.is_some() || limits.seven_day.is_some()).then_some(limits)
}

/// When to type the continuation: the reset of the window that stopped the
/// task, plus `jitter_secs`.
///
/// "The window that stopped it" is every window at 100%; when both are, the
/// later reset wins, since nothing runs until both are clear — that's the
/// weekly one in practice. When none reads 100% (the last status line update
/// can predate the refusal by a message), the fullest window is the one that
/// ran out.
pub fn resume_at(limits: &RateLimits, jitter_secs: i64) -> Option<DateTime<Utc>> {
    let windows: Vec<Window> = [limits.five_hour, limits.seven_day].into_iter().flatten().collect();
    let full: Vec<Window> = windows.iter().copied().filter(|w| w.used_percentage >= 100.0).collect();
    let stopping = if full.is_empty() {
        windows.iter().copied().max_by(|a, b| a.used_percentage.total_cmp(&b.used_percentage)).into_iter().collect()
    } else {
        full
    };
    let reset = stopping.iter().map(|w| w.resets_at).max()?;
    Utc.timestamp_opt(reset + jitter_secs, 0).single()
}

pub fn random_jitter() -> i64 {
    use rand::Rng;
    rand::thread_rng().gen_range(JITTER_SECS)
}

/// Whether this stretch of terminal text is Claude Code declining to wait.
/// Expects text with escape sequences already stripped (`plain_text`).
pub fn native_gave_up(text: &str) -> bool {
    text.to_lowercase().contains(NATIVE_GAVE_UP)
}

/// Terminal output with ANSI/OSC escape sequences removed, so a phrase the TUI
/// painted with colour or cursor moves reads as the words it is.
pub fn plain_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            if !c.is_control() || c == '\n' {
                out.push(c);
            }
            continue;
        }
        match chars.next() {
            // CSI: parameters, then one final byte in @..~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: until BEL or ST (ESC \).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Whether bytes the pane sent are the user typing, as opposed to the terminal
/// talking to the program on its own — focus in/out reports, which Claude Code
/// turns on, arrive through the same channel as keys and must not count.
pub fn is_user_input(data: &str) -> bool {
    !plain_text(data.as_bytes()).trim().is_empty() || data.contains('\r')
}

/// The status line wrapper every Hangar terminal's CLI runs.
///
/// Saves the payload where Hangar can read it (`rate_limits` carries the exact
/// reset times, and nothing else does), then hands the same payload to the
/// status line the account already had — a user's own, or Orca's — so the line
/// at the bottom of the CLI looks exactly as it did before.
const WRAPPER_SCRIPT: &str = r#"#!/bin/sh
# Written by Hangar. $1 is a file holding the account's own status line
# command, or empty when it has none.
input=$(cat)
if [ -n "$HANGAR_STATUS_KEY" ]; then
  out="$(dirname "$0")/$HANGAR_STATUS_KEY.json"
  printf '%s' "$input" > "$out.tmp" && mv "$out.tmp" "$out"
fi
if [ -n "$1" ] && [ -s "$1" ]; then
  printf '%s' "$input" | /bin/sh -c "$(cat "$1")"
fi
"#;

fn statusline_dir(root: &Path) -> PathBuf {
    root.join("statusline")
}

fn terminals_dir(root: &Path) -> PathBuf {
    root.join("terminals")
}

/// The environment variable the wrapper files its payload under.
pub const STATUS_KEY_ENV: &str = "HANGAR_STATUS_KEY";

/// Where an account's latest status line payload lands.
///
/// Per account, not per terminal: the limits belong to the subscription, and a
/// terminal restored after a restart comes back with a new id and a CLI that
/// hasn't reported anything yet — the account's last reading is the only
/// source of the reset time it was waiting for.
pub fn status_file(root: &Path, account_id: &str) -> PathBuf {
    statusline_dir(root).join(format!("{account_id}.json"))
}

/// The latest `rate_limits` any CLI of this account reported, if one did.
pub fn latest_rate_limits(root: &Path, account_id: &str) -> Option<RateLimits> {
    parse_rate_limits(&std::fs::read_to_string(status_file(root, account_id)).ok()?)
}

/// The status line command configured in an account's own `settings.json`,
/// unless it's Hangar's wrapper already (a settings file copied around must not
/// make the wrapper call itself).
fn original_status_line(config_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(config_dir.join("settings.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    let command = value.get("statusLine")?.get("command")?.as_str()?.trim().to_string();
    (!command.is_empty() && !command.contains("hangar-statusline.sh")).then_some(command)
}

fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// Writes the `--settings` file one terminal's CLI starts with, and returns its
/// path.
///
/// `autoContinueAtUsageLimit` is always written, never left out: absent means
/// *on* to Claude Code, and "off unless marked" is the whole point. The status
/// line is Unix-only — the wrapper is a shell script — so elsewhere the file
/// carries just the setting.
pub fn write_terminal_settings(root: &Path, terminal_id: &str, auto_continue: bool, config_dir: &Path) -> Result<PathBuf, String> {
    let dir = terminals_dir(root);
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let mut settings = serde_json::json!({ "autoContinueAtUsageLimit": auto_continue });

    if cfg!(unix) {
        let status_dir = statusline_dir(root);
        std::fs::create_dir_all(&status_dir).map_err(|e| format!("could not create {}: {e}", status_dir.display()))?;
        let wrapper = status_dir.join("hangar-statusline.sh");
        if std::fs::read_to_string(&wrapper).ok().as_deref() != Some(WRAPPER_SCRIPT) {
            std::fs::write(&wrapper, WRAPPER_SCRIPT).map_err(|e| format!("could not write {}: {e}", wrapper.display()))?;
        }
        let original = dir.join(format!("{terminal_id}.statusline"));
        match original_status_line(config_dir) {
            Some(command) => std::fs::write(&original, command).map_err(|e| format!("could not write {}: {e}", original.display()))?,
            None => {
                let _ = std::fs::remove_file(&original);
            }
        }
        let command = format!("/bin/sh {} {}", sh_quote(&wrapper.to_string_lossy()), sh_quote(&original.to_string_lossy()));
        settings["statusLine"] = serde_json::json!({ "type": "command", "command": command });
    }

    let path = dir.join(format!("{terminal_id}.settings.json"));
    let body = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// Drops the files written for a terminal that is gone. The account's status
/// reading stays: other terminals of the account, and a restore, still use it.
pub fn forget_terminal(root: &Path, terminal_id: &str) {
    let _ = std::fs::remove_file(terminals_dir(root).join(format!("{terminal_id}.settings.json")));
    let _ = std::fs::remove_file(terminals_dir(root).join(format!("{terminal_id}.statusline")));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(pct: f64, resets_at: i64) -> Option<Window> {
        Some(Window { used_percentage: pct, resets_at })
    }

    #[test]
    fn the_rate_limits_are_read_from_a_real_status_line_payload() {
        let payload = r#"{"model":{"id":"x"},"rate_limits":{"five_hour":{"used_percentage":100,"resets_at":1790000000},"seven_day":{"used_percentage":41.5,"resets_at":1790400000}}}"#;
        let limits = parse_rate_limits(payload).unwrap();
        assert_eq!(limits.five_hour, window(100.0, 1_790_000_000));
        assert_eq!(limits.seven_day, window(41.5, 1_790_400_000));
        assert_eq!(parse_rate_limits(r#"{"model":{"id":"x"}}"#), None, "an API-key account has no rate_limits");
        assert_eq!(parse_rate_limits(r#"{"rate_limits":{}}"#), None);
        assert_eq!(parse_rate_limits("not json"), None);
    }

    /// Nothing runs until every full window is clear, so the later reset wins —
    /// in practice the weekly one, days after the 5-hour window came back.
    #[test]
    fn the_later_of_the_full_windows_decides_when_to_continue() {
        let session_only = RateLimits { five_hour: window(100.0, 1_000), seven_day: window(60.0, 9_000) };
        assert_eq!(resume_at(&session_only, 150).unwrap().timestamp(), 1_150);

        let weekly_only = RateLimits { five_hour: window(30.0, 1_000), seven_day: window(100.0, 9_000) };
        assert_eq!(resume_at(&weekly_only, 150).unwrap().timestamp(), 9_150);

        let both = RateLimits { five_hour: window(100.0, 1_000), seven_day: window(100.0, 9_000) };
        assert_eq!(resume_at(&both, 150).unwrap().timestamp(), 9_150);
    }

    /// The last status line update can predate the refusal, so neither window
    /// reads 100% yet — the fullest one is the one that ran out.
    #[test]
    fn with_no_window_at_100_the_fullest_one_is_used() {
        let limits = RateLimits { five_hour: window(97.0, 1_000), seven_day: window(80.0, 9_000) };
        assert_eq!(resume_at(&limits, 0).unwrap().timestamp(), 1_000);
        assert_eq!(resume_at(&RateLimits::default(), 0), None);
    }

    #[test]
    fn the_jitter_lands_two_to_three_minutes_after_the_reset() {
        for _ in 0..200 {
            assert!(JITTER_SECS.contains(&random_jitter()));
        }
    }

    /// Both of Claude Code's give-up messages, as the TUI paints them — with
    /// colour codes in the middle of the words.
    #[test]
    fn both_native_give_up_messages_are_recognised_through_escape_codes() {
        let over_a_day = b"\x1b[33mthe usage limit now resets more than 24 hours out, so this task \x1b[1mwill not resume on its own\x1b[0m (/rate-limit-options to wait anyway)";
        let relaunched = b"Claude Code relaunched during the wait, so the task will not resume on its own when the usage limit resets";
        assert!(native_gave_up(&plain_text(over_a_day)));
        assert!(native_gave_up(&plain_text(relaunched)));
        assert!(!native_gave_up(&plain_text(b"Continuing automatically when your usage limit resets at 3pm")));
    }

    #[test]
    fn focus_reports_are_not_the_user_typing() {
        assert!(!is_user_input("\x1b[I"));
        assert!(!is_user_input("\x1b[O"));
        assert!(is_user_input("y"));
        assert!(is_user_input("\r"));
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-test-autocontinue-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Off unless marked, written out explicitly — absent reads as *on* to
    /// Claude Code.
    #[test]
    fn the_settings_file_always_states_the_choice() {
        let root = temp_dir("settings");
        let config = temp_dir("config");
        for on in [true, false] {
            let path = write_terminal_settings(&root, "t1", on, &config).unwrap();
            let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            assert_eq!(value["autoContinueAtUsageLimit"], serde_json::json!(on));
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&config);
    }

    /// The wrapper saves what Hangar needs and still draws the user's own
    /// status line — with no original command it saves and prints nothing.
    #[cfg(unix)]
    #[test]
    fn the_status_line_wrapper_saves_the_payload_and_chains_the_original() {
        let root = temp_dir("wrapper");
        let config = temp_dir("wrapper-config");
        std::fs::write(config.join("settings.json"), r#"{"statusLine":{"type":"command","command":"cat >/dev/null; printf 'ORIGINAL'"}}"#).unwrap();
        let payload = r#"{"rate_limits":{"five_hour":{"used_percentage":100,"resets_at":1790000000}}}"#;

        let run = |config: &Path| {
            let path = write_terminal_settings(&root, "t1", true, config).unwrap();
            let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let command = value["statusLine"]["command"].as_str().unwrap().to_string();
            let mut child = std::process::Command::new("/bin/sh")
                .args(["-c", &command])
                .env(STATUS_KEY_ENV, "acc1")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            use std::io::Write;
            child.stdin.take().unwrap().write_all(payload.as_bytes()).unwrap();
            String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
        };

        assert_eq!(run(&config), "ORIGINAL", "the account's own status line still draws");
        assert_eq!(latest_rate_limits(&root, "acc1").unwrap().five_hour, window(100.0, 1_790_000_000));

        let bare = temp_dir("wrapper-bare");
        assert_eq!(run(&bare), "", "no original command: nothing drawn, nothing failed");

        forget_terminal(&root, "t1");
        assert!(!root.join("terminals/t1.settings.json").exists());
        assert!(latest_rate_limits(&root, "acc1").is_some(), "the account's reading outlives the terminal");
        for dir in [&root, &config, &bare] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
