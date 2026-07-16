use crate::db::DbConnection;
use crate::process_manager::{emit_account_exhausted, emit_block, mark_session_status};
use crate::providers::{self, claude, Mode};
use rusqlite::params;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use tauri::{AppHandle, Manager, Runtime};

/// Tracks the PID of each in-flight one-shot `claude -p` turn, keyed by
/// session id, so `stop_session` can actually kill a hung/running turn.
/// (`process_manager::ProcessRegistry` only tracks the interactive-PTY
/// path — these headless turns never registered anywhere, so "Stop" had
/// nothing to kill and a deadlocked turn was unrecoverable short of
/// deleting the whole session.)
pub struct TurnRegistry(pub Mutex<HashMap<String, u32>>);

impl TurnRegistry {
    pub fn new() -> Self {
        TurnRegistry(Mutex::new(HashMap::new()))
    }
}

fn register_turn<R: Runtime>(app: &AppHandle<R>, session_id: &str, pid: u32) {
    if let Some(reg) = app.try_state::<TurnRegistry>() {
        if let Ok(mut map) = reg.0.lock() {
            map.insert(session_id.to_string(), pid);
        }
    }
}

fn unregister_turn<R: Runtime>(app: &AppHandle<R>, session_id: &str) {
    if let Some(reg) = app.try_state::<TurnRegistry>() {
        if let Ok(mut map) = reg.0.lock() {
            map.remove(session_id);
        }
    }
}

/// Kills an in-flight turn's process by PID, if one is tracked for this
/// session. Best-effort: used by `stop_session` to actually recover a
/// stuck/hung session instead of just flipping its DB status.
pub fn kill_turn<R: Runtime>(app: &AppHandle<R>, session_id: &str) {
    let pid = match app.try_state::<TurnRegistry>() {
        Some(reg) => match reg.0.lock() {
            Ok(mut map) => map.remove(session_id),
            Err(_) => None,
        },
        None => None,
    };
    let Some(pid) = pid else { return };

    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
    }
}

pub struct TurnRequest<R: Runtime = tauri::Wry> {
    pub app: AppHandle<R>,
    pub session_id: String,
    pub account_id: String,
    pub provider: String,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    pub mode: Mode,
    pub model: String,
    pub effort: String,
    pub name: String,
    pub resume: Option<String>,
    pub prompt: String,
}

/// Runs one headless turn (`claude -p "<prompt>" --output-format
/// stream-json`) in a background thread, parsing each JSON line into typed
/// session blocks (you/agent/think/edit/cmd/out) as it streams in. Multi-turn
/// continuity comes from `resume`, Claude Code's own conversation id
/// captured from the first turn's `system` message.
pub fn spawn_turn<R: Runtime>(req: TurnRequest<R>) {
    if req.provider != "claude" {
        emit_block(
            &req.app,
            &req.session_id,
            "agent",
            &format!("provider '{}' has no turn runner yet", req.provider),
        );
        mark_session_status(&req.app, &req.session_id, "waiting");
        return;
    }

    emit_block(&req.app, &req.session_id, "you", &req.prompt);
    mark_session_status(&req.app, &req.session_id, "running");

    let args = claude::build_turn_args(claude::TurnArgs {
        mode: req.mode,
        prompt: &req.prompt,
        resume: req.resume.as_deref(),
        model: &req.model,
        effort: &req.effort,
        name: &req.name,
    });

    std::thread::spawn(move || {
        let mut cmd = Command::new(claude::binary());
        cmd.args(&args)
            .current_dir(&req.cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &req.env {
            cmd.env(k, v);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                emit_block(&req.app, &req.session_id, "agent", &format!("failed to start claude: {e}"));
                mark_session_status(&req.app, &req.session_id, "waiting");
                return;
            }
        };
        register_turn(&req.app, &req.session_id, child.id());

        // Drain stderr on its own thread. Both pipes have a bounded OS
        // buffer (~64KB) — if the child writes enough to stderr while we're
        // only reading stdout, its write() blocks forever and the turn
        // hangs with the session stuck on "running" for good. This was a
        // real bug: `stderr(Stdio::piped())` was set but never read.
        let stderr_handle = child.stderr.take().map(|stderr| {
            let app = req.app.clone();
            let session_id = req.session_id.clone();
            let account_id = req.account_id.clone();
            std::thread::spawn(move || {
                let mut collected = String::new();
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { continue };
                    if line.trim().is_empty() {
                        continue;
                    }
                    if providers::is_exhaustion_signal("claude", &line) {
                        emit_account_exhausted(&app, &account_id, &session_id);
                    }
                    collected.push_str(&line);
                    collected.push('\n');
                }
                collected
            })
        });

        let mut stdout_blocks = 0usize;
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { continue };
                if line.trim().is_empty() {
                    continue;
                }
                stdout_blocks += apply_line(&req.app, &req.session_id, &req.account_id, &line);
            }
        }

        let exit_status = child.wait();
        unregister_turn(&req.app, &req.session_id);
        let stderr_text = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();

        // Never let a turn finish in total silence — this is the backstop
        // for every failure mode that isn't a clean stream-json `result`
        // line: crash before printing anything, killed by a signal, exited
        // 0 with a warning on stderr instead of stdout, etc. Whatever
        // stderr has is more useful than nothing; if even that's empty,
        // say so explicitly with the exit code instead of leaving the
        // panel blank with zero explanation.
        if stdout_blocks == 0 {
            if !stderr_text.trim().is_empty() {
                emit_block(&req.app, &req.session_id, "agent", stderr_text.trim());
            } else {
                let code_desc = match &exit_status {
                    Ok(status) => format!("exit code {}", status.code().map(|c| c.to_string()).unwrap_or_else(|| "unknown (killed by signal)".to_string())),
                    Err(e) => format!("could not wait on process: {e}"),
                };
                emit_block(
                    &req.app,
                    &req.session_id,
                    "agent",
                    &format!("claude produced no output ({code_desc}) — check that the CLI, account, and project path are all working."),
                );
            }
        }

        mark_session_status(&req.app, &req.session_id, "waiting");
    });
}

/// One session block produced by parsing a stream-json line, before it's
/// emitted/persisted — kept separate from `emit_block` so the parsing logic
/// is pure and unit-testable against real captured CLI output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBlock {
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineOutcome {
    pub blocks: Vec<ParsedBlock>,
    pub provider_session_id: Option<String>,
    pub exhausted: bool,
}

const MAX_TOOL_RESULT_CHARS: usize = 4000;

/// Parses one line of `claude --output-format stream-json` output. Pure and
/// side-effect free — see `apply_line` for the AppHandle-touching wrapper
/// used by the real turn runner.
pub fn parse_line(line: &str, provider: &str) -> LineOutcome {
    let mut outcome = LineOutcome::default();

    let Ok(value) = serde_json::from_str::<Value>(line) else {
        if providers::is_exhaustion_signal(provider, line) {
            outcome.exhausted = true;
        }
        // `--output-format stream-json` should mean every stdout line is
        // JSON, but a startup warning, deprecation notice, or crash trace
        // could still slip a plain-text line through — show it rather than
        // silently dropping it, since that's exactly how a real failure
        // reads as "nothing came back."
        outcome.blocks.push(ParsedBlock { kind: "agent", text: line.to_string() });
        return outcome;
    };

    let msg_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match msg_type {
        "system" => {
            if let Some(sid) = value.get("session_id").and_then(|v| v.as_str()) {
                outcome.provider_session_id = Some(sid.to_string());
            }
        }
        "assistant" => {
            let content = value.pointer("/message/content").and_then(|v| v.as_array());
            if let Some(content) = content {
                for item in content {
                    if let Some(block) = parse_content_item(item) {
                        outcome.blocks.push(block);
                    }
                }
            }
        }
        "user" => {
            // Tool results — the actual output of a Bash/Read/etc. call the
            // assistant just made. Dropping these (the original bug here)
            // makes the UI show commands running but never their output.
            let content = value.pointer("/message/content").and_then(|v| v.as_array());
            if let Some(content) = content {
                for item in content {
                    if item.get("type").and_then(|v| v.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let is_error = item.get("is_error").and_then(|v| v.as_bool()).unwrap_or(false);
                    let text = tool_result_text(item.get("content").unwrap_or(&Value::Null));
                    if text.trim().is_empty() {
                        continue;
                    }
                    let text = truncate(&text, MAX_TOOL_RESULT_CHARS);
                    outcome.blocks.push(ParsedBlock {
                        kind: "out",
                        text: if is_error { format!("error: {text}") } else { text },
                    });
                }
            }
        }
        "rate_limit_event" => {
            let status = value.pointer("/rate_limit_info/status").and_then(|v| v.as_str());
            if let Some(status) = status {
                if status != "allowed" {
                    outcome.exhausted = true;
                }
            }
        }
        "result" => {
            if let Some(text) = value.get("result").and_then(|v| v.as_str()) {
                outcome.blocks.push(ParsedBlock { kind: "agent", text: text.to_string() });
            }
            if providers::is_exhaustion_signal(provider, line) {
                outcome.exhausted = true;
            }
        }
        _ => {}
    }

    outcome
}

fn parse_content_item(item: &Value) -> Option<ParsedBlock> {
    let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match item_type {
        "text" => {
            let text = item.get("text").and_then(|v| v.as_str())?;
            if text.trim().is_empty() {
                return None;
            }
            Some(ParsedBlock { kind: "agent", text: text.to_string() })
        }
        "thinking" => {
            let text = item.get("thinking").and_then(|v| v.as_str())?;
            // Often empty (redacted, signature-only) on trivial turns — an
            // empty THINKING block is just noise.
            if text.trim().is_empty() {
                return None;
            }
            Some(ParsedBlock { kind: "think", text: text.to_string() })
        }
        "tool_use" => {
            let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
            let input = item.get("input").unwrap_or(&Value::Null);
            Some(format_tool_use(name, input))
        }
        _ => None,
    }
}

fn format_tool_use(name: &str, input: &Value) -> ParsedBlock {
    match name {
        "Bash" => {
            let command = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            ParsedBlock { kind: "cmd", text: format!("$ {command}") }
        }
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("(unknown file)");
            ParsedBlock { kind: "edit", text: format!("{name}  {path}") }
        }
        "Read" | "Glob" | "Grep" => {
            let target = input
                .get("file_path")
                .or_else(|| input.get("pattern"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            ParsedBlock { kind: "agent", text: format!("{name}  {target}") }
        }
        _ => ParsedBlock { kind: "agent", text: format!("{name} {}", compact_input(input)) },
    }
}

/// `tool_result.content` is either a plain string or an array of content
/// blocks (usually `{"type":"text","text":...}`, occasionally other shapes
/// depending on the tool) — normalize both into flat text.
fn tool_result_text(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    text.to_string()
                } else if let Some(name) = item.get("tool_name").and_then(|v| v.as_str()) {
                    name.to_string()
                } else {
                    value_to_plain(item)
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => value_to_plain(other),
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}\n… (truncated)")
}

fn compact_input(input: &Value) -> String {
    match input {
        Value::Object(map) => map
            .iter()
            .map(|(k, v)| format!("{k}={}", value_to_plain(v)))
            .collect::<Vec<_>>()
            .join(" "),
        other => value_to_plain(other),
    }
}

fn value_to_plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// AppHandle-touching wrapper around `parse_line`: emits/persists blocks,
/// stores the provider's own session id, and flags account exhaustion.
/// Returns how many blocks were emitted, so the caller can tell "the CLI
/// ran and said nothing" apart from "the CLI never produced valid output"
/// (the latter is when stderr should be surfaced as a fallback).
fn apply_line<R: Runtime>(app: &AppHandle<R>, session_id: &str, account_id: &str, line: &str) -> usize {
    let outcome = parse_line(line, "claude");

    for block in &outcome.blocks {
        emit_block(app, session_id, block.kind, &block.text);
    }
    if let Some(provider_session_id) = &outcome.provider_session_id {
        store_provider_session_id(app, session_id, provider_session_id);
    }
    if outcome.exhausted {
        emit_account_exhausted(app, account_id, session_id);
    }
    outcome.blocks.len()
}

fn store_provider_session_id<R: Runtime>(app: &AppHandle<R>, session_id: &str, provider_session_id: &str) {
    if let Some(db) = app.try_state::<DbConnection>() {
        match db.0.lock() {
            Ok(conn) => {
                if let Err(e) = conn.execute(
                    "UPDATE sessions SET provider_session_id = ?2 WHERE id = ?1",
                    params![session_id, provider_session_id],
                ) {
                    eprintln!("store_provider_session_id: failed for session {session_id}: {e}");
                }
            }
            Err(e) => eprintln!("store_provider_session_id: DB mutex poisoned for session {session_id}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_fixture(path: &str) -> Vec<LineOutcome> {
        let content = std::fs::read_to_string(path).expect("read fixture");
        content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| parse_line(l, "claude"))
            .collect()
    }

    /// Real captured output from `claude -p "..." --output-format
    /// stream-json --verbose --permission-mode acceptEdits` — a plain
    /// single-turn exchange with no tool calls.
    #[test]
    fn parses_real_simple_turn_fixture() {
        let outcomes = parse_fixture("tests/fixtures/real_turn_simple.jsonl");

        let session_id = outcomes.iter().find_map(|o| o.provider_session_id.clone());
        assert_eq!(session_id, Some("8ec63d6c-fe51-400c-817d-05ffa94a893e".to_string()));

        let all_blocks: Vec<_> = outcomes.iter().flat_map(|o| o.blocks.iter()).collect();
        // The final `result` message's text must come through as an agent block.
        assert!(all_blocks.iter().any(|b| b.kind == "agent" && b.text.contains("Fleet Agent project directory")));
        assert!(!outcomes.iter().any(|o| o.exhausted));
    }

    /// Real captured output from a `sub`-billed account whose OAuth login
    /// had actually expired/never completed — the CLI exits 1 immediately
    /// with `is_error:true` but *does* print a valid `result` message.
    /// Confirms the "account isn't really logged in" case surfaces a real,
    /// readable block instead of leaving the panel blank.
    #[test]
    fn parses_real_not_logged_in_fixture() {
        let outcomes = parse_fixture("tests/fixtures/real_turn_not_logged_in.jsonl");
        let all_blocks: Vec<_> = outcomes.iter().flat_map(|o| o.blocks.iter()).collect();
        assert!(
            all_blocks.iter().any(|b| b.kind == "agent" && b.text.contains("Not logged in")),
            "expected the 'Not logged in' result text to surface as a block, got: {all_blocks:?}"
        );
    }

    /// Real captured output including tool_use (Bash + ToolSearch) and
    /// tool_result (both plain-string and array-shaped content) — this is
    /// the fixture that exercises the original missing-`"user"`-type bug.
    #[test]
    fn parses_real_turn_with_tool_calls_fixture() {
        let outcomes = parse_fixture("tests/fixtures/real_turn_with_tools.jsonl");
        let all_blocks: Vec<ParsedBlock> = outcomes.into_iter().flat_map(|o| o.blocks).collect();

        // The Bash command itself renders as a clean `$ <command>` RUN block.
        assert!(
            all_blocks
                .iter()
                .any(|b| b.kind == "cmd" && b.text.starts_with("$ ") && b.text.contains("hello.txt")),
            "expected a cmd block with the bash command, got: {all_blocks:?}"
        );

        // The tool_result of that Bash call — the actual output — must be
        // present. This is exactly what the old parser silently dropped.
        assert!(
            all_blocks.iter().any(|b| b.kind == "out" && b.text.contains("hello fleet")),
            "expected an out block with the bash tool_result, got: {all_blocks:?}"
        );

        // A tool_result with array-shaped content ("No matching..."/tool_reference
        // items) must still produce readable text, not be dropped or panic.
        assert!(all_blocks.iter().any(|b| b.kind == "out" && b.text.contains("No matching deferred tools found")));

        // Final assistant summary text.
        assert!(all_blocks.iter().any(|b| b.kind == "agent" && b.text.contains("hello fleet")));

        // No empty THINKING blocks despite every "thinking" field being "".
        assert!(!all_blocks.iter().any(|b| b.kind == "think"));
    }

    #[test]
    fn formats_bash_as_shell_prompt_without_description_noise() {
        let input = serde_json::json!({"command": "pnpm test services/datasource", "description": "run tests"});
        let block = format_tool_use("Bash", &input);
        assert_eq!(block.kind, "cmd");
        assert_eq!(block.text, "$ pnpm test services/datasource");
    }

    #[test]
    fn formats_edit_family_as_just_the_file_path() {
        let input = serde_json::json!({"file_path": "src/main.rs", "old_string": "a", "new_string": "b"});
        let block = format_tool_use("Edit", &input);
        assert_eq!(block.kind, "edit");
        assert_eq!(block.text, "Edit  src/main.rs");
        // Must not leak the raw diff content into the summary line.
        assert!(!block.text.contains("old_string"));
    }

    #[test]
    fn rate_limit_event_not_allowed_marks_exhausted() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"rejected"}}"#;
        let outcome = parse_line(line, "claude");
        assert!(outcome.exhausted);
        assert!(outcome.blocks.is_empty());
    }

    #[test]
    fn rate_limit_event_allowed_does_not_mark_exhausted() {
        let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}"#;
        let outcome = parse_line(line, "claude");
        assert!(!outcome.exhausted);
    }

    #[test]
    fn non_json_stdout_line_still_surfaces_instead_of_vanishing() {
        let outcome = parse_line("npm WARN deprecated some-package@1.0.0", "claude");
        assert_eq!(outcome.blocks.len(), 1);
        assert_eq!(outcome.blocks[0].kind, "agent");
        assert!(outcome.blocks[0].text.contains("npm WARN deprecated"));
    }

    #[test]
    fn tool_result_array_with_tool_reference_items_renders_names() {
        let input = serde_json::json!({
            "content": [
                {"type": "tool_reference", "tool_name": "ExitWorktree"},
                {"type": "tool_reference", "tool_name": "TaskCreate"}
            ]
        });
        let text = tool_result_text(input.get("content").unwrap());
        assert!(text.contains("ExitWorktree"));
        assert!(text.contains("TaskCreate"));
    }

    #[test]
    fn long_tool_result_gets_truncated() {
        let huge = "x".repeat(MAX_TOOL_RESULT_CHARS + 500);
        let truncated = truncate(&huge, MAX_TOOL_RESULT_CHARS);
        assert!(truncated.ends_with("… (truncated)"));
        assert!(truncated.len() < huge.len());
    }

    /// Real, end-to-end reproduction of the "I send a message and nothing
    /// shows up in the panel" report — drives the actual `spawn_turn` code
    /// path (not just `parse_line`) against a real `tauri::test::mock_app`
    /// and a real `claude` invocation, with no login in the isolated config
    /// dir (matching the real account's actual state at the time). If this
    /// ever goes back to producing zero blocks, this test catches it
    /// directly instead of relying on manual DB inspection again.
    #[test]
    fn spawn_turn_writes_you_block_and_final_result_even_when_not_logged_in() {
        use rusqlite::Connection;
        use std::sync::Mutex as StdMutex;
        use std::time::{Duration, Instant};

        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();

        let isolated_config_dir = std::env::temp_dir().join(format!("fleet-test-turn-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&isolated_config_dir).unwrap();

        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        ).unwrap();
        let project_path = env!("CARGO_MANIFEST_DIR").to_string();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test',?1,'now')",
            [&project_path],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('sess1','test-session','proj1','acc1','','high','ask_edits','waiting','now')",
            [],
        ).unwrap();

        app.manage(crate::db::DbConnection(StdMutex::new(conn)));
        app.manage(TurnRegistry::new());

        spawn_turn(TurnRequest {
            app: app.handle().clone(),
            session_id: "sess1".to_string(),
            account_id: "acc1".to_string(),
            provider: "claude".to_string(),
            cwd: project_path,
            env: vec![(
                "CLAUDE_CONFIG_DIR".to_string(),
                isolated_config_dir.to_string_lossy().to_string(),
            )],
            mode: Mode::AskEdits,
            model: String::new(),
            effort: String::new(),
            name: String::new(),
            resume: None,
            prompt: "say hello".to_string(),
        });

        // The "you" block is emitted synchronously before the background
        // thread is even spawned — if this isn't there almost immediately,
        // nothing downstream matters.
        let db = app.state::<crate::db::DbConnection>();
        let you_block: String = db
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT text FROM session_blocks WHERE session_id = 'sess1' AND kind = 'you'",
                [],
                |r| r.get(0),
            )
            .expect("the 'you' block must be written synchronously, before the CLI even spawns");
        assert_eq!(you_block, "say hello");

        // The actual `claude` process runs in a background thread — poll
        // for it to finish (real subprocess, real exit) instead of a fixed
        // sleep.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut final_status = String::new();
        while Instant::now() < deadline {
            final_status = db
                .0
                .lock()
                .unwrap()
                .query_row("SELECT status FROM sessions WHERE id = 'sess1'", [], |r| r.get(0))
                .unwrap();
            if final_status == "waiting" {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(final_status, "waiting", "turn must return the session to 'waiting' once it completes");

        let all_text: Vec<String> = db
            .0
            .lock()
            .unwrap()
            .prepare("SELECT text FROM session_blocks WHERE session_id = 'sess1' ORDER BY created_at")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(
            all_text.iter().any(|t| t.contains("Not logged in") || t.contains("logged in")),
            "expected a real 'not logged in' response from the unauthenticated config dir, got blocks: {all_text:?}"
        );

        let _ = std::fs::remove_dir_all(&isolated_config_dir);
    }
}
