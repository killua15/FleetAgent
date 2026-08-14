use crate::db::DbConnection;
use crate::permission_bridge;
use crate::process_manager::{emit_account_exhausted, emit_block, mark_session_status};
use crate::providers::{self, claude, Mode};
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, Runtime};

/// Tracks each in-flight one-shot `claude -p` turn, keyed by session id, so
/// `stop_session` can actually kill a hung/running turn.
/// (`process_manager::ProcessRegistry` only tracks the interactive-PTY
/// path — these headless turns never registered anywhere, so "Stop" had
/// nothing to kill and a deadlocked turn was unrecoverable short of
/// deleting the whole session.)
///
/// The value is `None` between `try_begin_turn` (which reserves the slot
/// synchronously, before the background thread even starts) and
/// `register_turn` (which fills in the real pid once the child is spawned).
/// That reservation window is what makes the "one turn per session" guard
/// actually atomic: registering only after `Command::spawn` left a gap where
/// two rapid `send_message` calls both saw an empty registry and both
/// launched a `claude --resume <same id>` against the same on-disk
/// transcript, with the first pid then silently overwritten and orphaned.
pub struct TurnRegistry(pub Mutex<HashMap<String, Option<u32>>>);

impl TurnRegistry {
    pub fn new() -> Self {
        TurnRegistry(Mutex::new(HashMap::new()))
    }
}

/// Sessions whose in-flight turn the user explicitly aborted via
/// `stop_session`, consumed once by that turn's own tail in `spawn_turn`.
///
/// Without this, pressing Stop undid itself: `stop_session` writes
/// `status = 'done'`, the killed child's stdout hits EOF, and the turn thread
/// runs its normal tail — `mark_session_status(..., "waiting")` — putting the
/// session straight back to `waiting` and firing a "Session finished"
/// notification for a turn the user had just cancelled. A cancelled turn now
/// leaves both the status and the notification alone.
pub struct CancelledTurns(pub Mutex<std::collections::HashSet<String>>);

impl CancelledTurns {
    pub fn new() -> Self {
        CancelledTurns(Mutex::new(std::collections::HashSet::new()))
    }
}

/// Accounts already notified about a spend-cap breach this app run — see
/// `check_spend_cap`. In-memory only; resets on restart.
pub struct SpendCapNotified(pub Mutex<std::collections::HashSet<String>>);

impl SpendCapNotified {
    pub fn new() -> Self {
        SpendCapNotified(Mutex::new(std::collections::HashSet::new()))
    }
}

/// Atomically reserves this session's single turn slot. Returns `false` if a
/// turn is already in flight for it, which is the caller's cue to refuse
/// rather than start a second concurrent `claude` against the same
/// conversation. Reserving here — synchronously, before any work happens —
/// rather than at `register_turn` time is what closes the double-spawn race
/// described on `TurnRegistry`.
fn try_begin_turn<R: Runtime>(app: &AppHandle<R>, session_id: &str) -> bool {
    let Some(reg) = app.try_state::<TurnRegistry>() else {
        // No registry managed (only reachable in tests that never set one up)
        // — nothing to serialize against, so don't block the turn.
        return true;
    };
    let Ok(mut map) = reg.0.lock() else { return false };
    if map.contains_key(session_id) {
        return false;
    }
    map.insert(session_id.to_string(), None);
    true
}

fn register_turn<R: Runtime>(app: &AppHandle<R>, session_id: &str, pid: u32) {
    if let Some(reg) = app.try_state::<TurnRegistry>() {
        if let Ok(mut map) = reg.0.lock() {
            map.insert(session_id.to_string(), Some(pid));
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

/// Consumes the "this turn was cancelled by the user" flag for a session.
/// Consuming (rather than peeking) matters: the flag must never outlive the
/// one turn it was set for, or the *next* turn on that session would wrongly
/// suppress its own completion.
fn take_cancelled<R: Runtime>(app: &AppHandle<R>, session_id: &str) -> bool {
    let Some(state) = app.try_state::<CancelledTurns>() else { return false };
    let Ok(mut set) = state.0.lock() else { return false };
    set.remove(session_id)
}

/// Kills an in-flight turn's process by PID, if one is tracked for this
/// session, and flags the turn as user-cancelled so its own tail leaves the
/// session status alone (see `CancelledTurns`). Returns whether there was
/// actually a turn to stop. Best-effort: used by `stop_session` to recover a
/// stuck/hung session instead of just flipping its DB status.
pub fn kill_turn<R: Runtime>(app: &AppHandle<R>, session_id: &str) -> bool {
    let entry = match app.try_state::<TurnRegistry>() {
        Some(reg) => match reg.0.lock() {
            Ok(mut map) => map.remove(session_id),
            Err(_) => None,
        },
        None => None,
    };
    // `None` = no turn in flight at all. `Some(None)` = a turn slot was
    // reserved but its child hasn't spawned yet — nothing to kill, but it
    // still counts as cancelled so the turn about to start bails out.
    let Some(pid) = entry else { return false };
    if let Some(state) = app.try_state::<CancelledTurns>() {
        if let Ok(mut set) = state.0.lock() {
            set.insert(session_id.to_string());
        }
    }
    if let Some(pid) = pid {
        kill_pid_with_grace(pid);
    }
    true
}

/// Kills every in-flight turn, used on app exit so a quit doesn't leave
/// orphaned `claude` processes running against the user's repos with nothing
/// left to collect their output. Drains the registry as it goes.
pub fn kill_all_turns<R: Runtime>(app: &AppHandle<R>) {
    let Some(reg) = app.try_state::<TurnRegistry>() else { return };
    let Ok(mut map) = reg.0.lock() else { return };
    for (session_id, pid) in map.drain() {
        if let Some(pid) = pid {
            log::info!("killing in-flight turn for session {session_id} (pid {pid}) on exit");
            kill_pid_with_grace(pid);
        }
    }
}

#[cfg(unix)]
fn pid_is_alive(pid: u32) -> bool {
    // `kill -0` sends no actual signal — it's a pure liveness/permission
    // probe, the standard Unix way to ask "does this pid still exist?"
    Command::new("kill").args(["-0", &pid.to_string()]).status().map(|s| s.success()).unwrap_or(false)
}

/// Same theory as the post-turn kill in `spawn_turn`: forcing the CLI down
/// with zero warning risks catching it mid-write to its own on-disk
/// resumable-conversation transcript — corrupting every future `--resume`
/// of this same session — whether that kill is the automatic end-of-turn
/// one or (here) the user pressing "Stop" on a turn that looks stuck.
/// SIGTERM first gives a well-behaved process a chance to catch it and
/// flush/close files before exiting; a short grace window, then SIGKILL,
/// guarantees it actually goes down either way.
fn kill_pid_with_grace(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).output();
        let deadline = Instant::now() + Duration::from_millis(800);
        while Instant::now() < deadline {
            if !pid_is_alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
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
    /// The user's standing preferences (conversation and code language), as a
    /// file for `--append-system-prompt-file`, or `None` when nothing is set.
    ///
    /// Resolved by the caller rather than here: this runner has no database
    /// handle and gains nothing from one — every caller already knows which
    /// project the turn belongs to.
    pub instructions: Option<std::path::PathBuf>,
    /// The `--mcp-config` naming this project's code graph, or `None` when
    /// there is no graph (the common case). Resolved by the caller for the
    /// same reason as `instructions`.
    pub mcp_config: Option<std::path::PathBuf>,
}

/// Runs one headless turn (`claude -p "<prompt>" --output-format
/// stream-json`) in a background thread, parsing each JSON line into typed
/// session blocks (you/agent/think/edit/cmd/out) as it streams in. Multi-turn
/// continuity comes from `resume`, Claude Code's own conversation id
/// captured from the first turn's `system` message.
pub fn spawn_turn<R: Runtime>(req: TurnRequest<R>) -> Result<(), String> {
    // Reserve this session's turn slot before anything else — including
    // before the "you" block is emitted, so a rejected second message never
    // leaves a stray prompt in the transcript for a turn that never ran.
    if !try_begin_turn(&req.app, &req.session_id) {
        return Err("this session already has a turn running — wait for it to finish or press Stop".to_string());
    }

    if req.provider != "claude" {
        emit_block(
            &req.app,
            &req.session_id,
            "agent",
            &format!("provider '{}' has no turn runner yet", req.provider),
        );
        unregister_turn(&req.app, &req.session_id);
        mark_session_status(&req.app, &req.session_id, "waiting");
        return Ok(());
    }

    emit_block(&req.app, &req.session_id, "you", &req.prompt);
    mark_session_status(&req.app, &req.session_id, "running");

    // AskEdits (`--permission-mode default`) and AutoEdit (`acceptEdits`)
    // can both still block on approval in headless `-p` mode with no way to
    // answer it — acceptEdits auto-approves file edits but NOT arbitrary
    // Bash commands (verified against a real `claude` install: running a
    // build script under acceptEdits hits the same "requires approval" dead
    // end as default mode). Claude Code's own stdio control protocol
    // (`--permission-prompt-tool stdio`, wired up by `build_turn_args` below)
    // is the real approval channel for both — Hangar answers `control_request`
    // lines directly over this same child's stdin/stdout further down. Plan
    // never reaches this (the model explores read-only instead of invoking
    // risky commands) and Yolo (`bypassPermissions`) never blocks in the
    // first place, so neither needs it.
    let use_permission_prompt = matches!(req.mode, Mode::AskEdits | Mode::AutoEdit);
    // AutoEdit still needs the control protocol wired up (acceptEdits alone
    // doesn't cover Bash), but the user explicitly wants "auto" to mean
    // auto — Hangar answers every control_request itself with "allow" and
    // never shows the banner, matching the name instead of Claude Code's
    // own stricter default for acceptEdits. Only AskEdits still asks a human.
    let auto_approve = req.mode == Mode::AutoEdit;

    let args = claude::build_turn_args(claude::TurnArgs {
        mode: req.mode,
        prompt: &req.prompt,
        resume: req.resume.as_deref(),
        model: &req.model,
        effort: &req.effort,
        name: &req.name,
        use_permission_prompt,
        instructions: req.instructions.as_deref(),
        mcp_config: req.mcp_config.as_deref(),
    });

    std::thread::spawn(move || {
        let mut cmd = Command::new(claude::binary());
        cmd.args(&args)
            .current_dir(&req.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (k, v) in &req.env {
            cmd.env(k, v);
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                emit_block(&req.app, &req.session_id, "agent", &format!("failed to start claude: {e}"));
                // Release the slot reserved by `try_begin_turn` — without
                // this, a session whose very first spawn failed would be
                // permanently wedged, refusing every later message as
                // "already running".
                unregister_turn(&req.app, &req.session_id);
                let _ = take_cancelled(&req.app, &req.session_id);
                mark_session_status(&req.app, &req.session_id, "waiting");
                return;
            }
        };
        register_turn(&req.app, &req.session_id, child.id());

        // With `use_permission_prompt`, `build_turn_args` left the prompt off
        // the command line (`--input-format stream-json` expects it as the
        // first stdin message instead) and this same stdin doubles as the
        // channel `control_response` lines go back over, further down.
        let mut stdin = child.stdin.take();
        if use_permission_prompt {
            if let Some(stdin) = &mut stdin {
                let initial = json!({
                    "type": "user",
                    "message": { "role": "user", "content": [{ "type": "text", "text": req.prompt }] },
                })
                .to_string();
                let _ = writeln!(stdin, "{initial}");
            }
        }

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
        let mut saw_final_result = false;
        // Tracks the most recently emitted 'agent' block (id, text) across
        // this whole turn — `apply_line` uses it to detect the terminal
        // `result` message echoing the same text as the assistant's own
        // last response (see its doc comment) instead of emitting a
        // duplicate block for it.
        let mut last_agent_block: Option<(String, String)> = None;
        if let Some(stdout) = child.stdout.take() {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { continue };
                if line.trim().is_empty() {
                    continue;
                }

                if use_permission_prompt {
                    if let Some(control_req) = permission_bridge::parse_control_request(&line) {
                        let allow = if auto_approve { true } else { permission_bridge::await_decision(&req.app, &req.session_id, &control_req) };
                        if let Some(stdin) = &mut stdin {
                            let response = permission_bridge::build_control_response(&control_req.request_id, allow);
                            let _ = writeln!(stdin, "{response}");
                        }
                        continue;
                    }
                }

                stdout_blocks += apply_line(&req.app, &req.session_id, &req.account_id, &line, &mut last_agent_block);

                // `--input-format stream-json` (only set when
                // `use_permission_prompt`) keeps the CLI alive waiting for
                // more turns even after stdin closes — verified empirically:
                // it stayed running well past its final `result` line with
                // nothing left to say. Hangar's model is one process per
                // turn, so the terminal `result` line IS the end of this
                // turn; kill the process here instead of waiting on an exit
                // that will never come on its own.
                if use_permission_prompt && is_final_result_line(&line) {
                    saw_final_result = true;
                    break;
                }
            }
        }
        if saw_final_result {
            // `--input-format stream-json` never exits on its own after its
            // final `result` line (verified empirically — see the comment
            // above), so it still has to be killed. But killing the instant
            // that line is read off stdout risks catching the CLI mid-write
            // to its own on-disk resumable-conversation transcript (the file
            // `--resume <provider_session_id>` reads back on the next
            // turn) — give it a short window to finish any such bookkeeping
            // and exit on its own first, and only force it down if it doesn't.
            let deadline = Instant::now() + Duration::from_millis(800);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
                    _ => {
                        let _ = child.kill();
                        break;
                    }
                }
            }
        }

        let exit_status = child.wait();
        unregister_turn(&req.app, &req.session_id);
        let stderr_text = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();

        // A turn the user aborted via `stop_session` must not run its normal
        // tail: `stop_session` already wrote `status = 'done'`, and both the
        // silence backstop ("claude produced no output (killed by signal)")
        // and the `waiting` transition below would be actively wrong — the
        // former reports the user's own Stop as a failure, the latter undoes
        // it outright and fires a "Session finished" notification for a turn
        // that never finished. See `CancelledTurns`.
        if take_cancelled(&req.app, &req.session_id) {
            log::info!("turn for session {} was cancelled by the user; leaving its status alone", req.session_id);
            return;
        }

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

    Ok(())
}

/// One session block produced by parsing a stream-json line, before it's
/// emitted/persisted — kept separate from `emit_block` so the parsing logic
/// is pure and unit-testable against real captured CLI output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedBlock {
    pub kind: &'static str,
    pub text: String,
}

// `PartialEq` only (not `Eq`) — `turn_usage.cost_usd` is an `f64`, which has
// no total ordering/equality in Rust and can't derive `Eq`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineOutcome {
    pub blocks: Vec<ParsedBlock>,
    pub provider_session_id: Option<String>,
    pub exhausted: bool,
    pub file_edits: Vec<FileEditCapture>,
    pub turn_usage: Option<TurnUsageCapture>,
}

/// The real before/after content of one Edit/Write/MultiEdit tool call —
/// captured separately from `ParsedBlock` (which only keeps a human-readable
/// "Edit path" summary for the timeline) so a diff view has real text to
/// render instead of just the path that changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEditCapture {
    pub file_path: String,
    pub tool_name: String,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
    /// Index into the same `LineOutcome::blocks` this capture was produced
    /// alongside (the "Edit path" summary block for the same tool_use item) —
    /// lets `apply_line` resolve the real persisted block id to link
    /// `file_edits.block_id` to. `None` if set outside of `parse_line`'s
    /// normal per-item loop (shouldn't happen in practice, but keeps the
    /// field honest rather than defaulting to a fake index).
    pub source_block_index: Option<usize>,
}

/// Token/cost accounting from a turn's final `"result"` message — captured
/// alongside the `agent` block it summarizes, the same way `FileEditCapture`
/// links to the `edit` block that produced it.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnUsageCapture {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub source_block_index: Option<usize>,
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
                    let pushed_index = if let Some(block) = parse_content_item(item) {
                        outcome.blocks.push(block);
                        Some(outcome.blocks.len() - 1)
                    } else {
                        None
                    };
                    if item.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("tool");
                        let input = item.get("input").unwrap_or(&Value::Null);
                        let mut edits = capture_file_edits(name, input);
                        for edit in &mut edits {
                            edit.source_block_index = pushed_index;
                        }
                        outcome.file_edits.extend(edits);
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
            let mut source_block_index = None;
            if let Some(text) = value.get("result").and_then(|v| v.as_str()) {
                // Claude Code can end a turn with an empty `result` string in
                // abort/error conditions — an empty AGENT block would look
                // like a real (if silent) response instead of the failure it
                // actually is, and would suppress the "never finish in total
                // silence" backstop in `spawn_turn` below, which needs
                // `stdout_blocks` to stay at 0 to kick in. Mirrors the same
                // empty-text guard `parse_content_item`'s "text"/"thinking"
                // arms already use.
                if !text.trim().is_empty() {
                    outcome.blocks.push(ParsedBlock { kind: "agent", text: text.to_string() });
                    source_block_index = Some(outcome.blocks.len() - 1);
                }
            }
            if let Some(usage) = value.get("usage") {
                let input_tokens = usage.get("input_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                let output_tokens = usage.get("output_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
                outcome.turn_usage = Some(TurnUsageCapture {
                    input_tokens,
                    output_tokens,
                    cache_read_tokens: usage.get("cache_read_input_tokens").and_then(|v| v.as_i64()),
                    cache_creation_tokens: usage.get("cache_creation_input_tokens").and_then(|v| v.as_i64()),
                    cost_usd: value.get("total_cost_usd").and_then(|v| v.as_f64()),
                    source_block_index,
                });
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

/// Extracts the real before/after text of an Edit/Write/MultiEdit tool call,
/// if any — a `MultiEdit` call can rewrite the same file several times in one
/// go, so this returns one capture per individual replacement rather than
/// collapsing them into one. Any other tool (including NotebookEdit, whose
/// input shape differs — `notebook_path`/`new_source` instead of
/// `file_path`/`old_string`/`new_string` — is out of scope for now) yields
/// nothing.
fn capture_file_edits(name: &str, input: &Value) -> Vec<FileEditCapture> {
    let Some(file_path) = input.get("file_path").and_then(|v| v.as_str()) else {
        return Vec::new();
    };
    match name {
        "Edit" => {
            let old_text = input.get("old_string").and_then(|v| v.as_str()).map(str::to_string);
            let new_text = input.get("new_string").and_then(|v| v.as_str()).map(str::to_string);
            vec![FileEditCapture { file_path: file_path.to_string(), tool_name: name.to_string(), old_text, new_text, source_block_index: None }]
        }
        "Write" => {
            let new_text = input.get("content").and_then(|v| v.as_str()).map(str::to_string);
            vec![FileEditCapture { file_path: file_path.to_string(), tool_name: name.to_string(), old_text: None, new_text, source_block_index: None }]
        }
        "MultiEdit" => {
            let Some(edits) = input.get("edits").and_then(|v| v.as_array()) else {
                return Vec::new();
            };
            edits
                .iter()
                .map(|e| FileEditCapture {
                    file_path: file_path.to_string(),
                    tool_name: name.to_string(),
                    old_text: e.get("old_string").and_then(|v| v.as_str()).map(str::to_string),
                    new_text: e.get("new_string").and_then(|v| v.as_str()).map(str::to_string),
                    source_block_index: None,
                })
                .collect()
        }
        _ => Vec::new(),
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
/// Cheap check for `claude --output-format stream-json`'s terminal message
/// (`{"type":"result",...}`) — a separate pass from `parse_line`/`apply_line`
/// since the caller needs to know this *before* deciding whether to keep
/// reading, not just what block to emit for it.
fn is_final_result_line(line: &str) -> bool {
    serde_json::from_str::<Value>(line).ok().and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string)).as_deref() == Some("result")
}

/// `last_agent_block` tracks the most recently emitted 'agent' block (id,
/// text) across the whole turn — see the dedup comment below for why.
fn apply_line<R: Runtime>(app: &AppHandle<R>, session_id: &str, account_id: &str, line: &str, last_agent_block: &mut Option<(String, String)>) -> usize {
    let outcome = parse_line(line, "claude");
    let is_result_line = is_final_result_line(line);

    let mut block_ids: Vec<String> = Vec::with_capacity(outcome.blocks.len());
    for block in &outcome.blocks {
        // Claude Code's terminal `result` message typically echoes the
        // exact text of the assistant's own last response verbatim in its
        // `result` field (confirmed against real captured output — see
        // `tests/fixtures/real_turn_simple.jsonl`) — emitting a second
        // AGENT block for it would just duplicate the same response in the
        // transcript. Only ever suppressed for the `result` line's own
        // block (never an ordinary assistant text block) and only when
        // it's a byte-for-byte repeat of the immediately preceding one, so
        // a turn with no prior text (or a result that's genuinely
        // different) still surfaces normally.
        if is_result_line && block.kind == "agent" {
            if let Some((existing_id, existing_text)) = last_agent_block.as_ref() {
                if existing_text == &block.text {
                    block_ids.push(existing_id.clone());
                    continue;
                }
            }
        }
        let id = emit_block(app, session_id, block.kind, &block.text);
        if block.kind == "agent" {
            *last_agent_block = Some((id.clone(), block.text.clone()));
        }
        block_ids.push(id);
    }
    for file_edit in &outcome.file_edits {
        let block_id = file_edit.source_block_index.and_then(|i| block_ids.get(i)).map(String::as_str);
        store_file_edit(app, session_id, file_edit, block_id);
    }
    if let Some(usage) = &outcome.turn_usage {
        let block_id = usage.source_block_index.and_then(|i| block_ids.get(i)).map(String::as_str);
        store_turn_usage(app, session_id, usage, block_id);
    }
    if let Some(provider_session_id) = &outcome.provider_session_id {
        store_provider_session_id(app, session_id, provider_session_id);
    }
    if outcome.exhausted {
        emit_account_exhausted(app, account_id, session_id);
    }
    outcome.blocks.len()
}

fn store_file_edit<R: Runtime>(app: &AppHandle<R>, session_id: &str, file_edit: &FileEditCapture, block_id: Option<&str>) {
    if let Some(db) = app.try_state::<DbConnection>() {
        match db.0.lock() {
            Ok(conn) => {
                if let Err(e) = conn.execute(
                    "INSERT INTO file_edits (id, session_id, block_id, file_path, tool_name, old_text, new_text, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        session_id,
                        block_id,
                        file_edit.file_path,
                        file_edit.tool_name,
                        file_edit.old_text,
                        file_edit.new_text,
                        chrono::Utc::now().to_rfc3339(),
                    ],
                ) {
                    log::error!("store_file_edit: failed to persist edit for session {session_id}: {e}");
                }
            }
            Err(e) => log::error!("store_file_edit: DB mutex poisoned for session {session_id}: {e}"),
        }
    }
}

fn store_turn_usage<R: Runtime>(app: &AppHandle<R>, session_id: &str, usage: &TurnUsageCapture, block_id: Option<&str>) {
    if let Some(db) = app.try_state::<DbConnection>() {
        match db.0.lock() {
            Ok(conn) => {
                if let Err(e) = conn.execute(
                    // `project_id` comes off the session rather than being left
                    // NULL as it was: every per-project report — spend, budget,
                    // and now the graph comparison — filters on it, so headless
                    // turns were invisible to all of them.
                    // The names are resolved here, at write time, and stored on
                    // the row: `turn_usage` is a ledger, and a name looked up by
                    // JOIN when the report runs disappears the moment somebody
                    // deletes the project or the account (see migration 26).
                    // `account_id` comes off the session for the same reason
                    // `project_id` does.
                    "INSERT INTO turn_usage (id, session_id, project_id, account_id, block_id,
                                             input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens,
                                             cost_usd, created_at, with_graph,
                                             project_name, account_name, provider, machine_id, machine_name)
                     VALUES (?1, ?2,
                             (SELECT project_id FROM sessions WHERE id = ?2),
                             (SELECT account_id FROM sessions WHERE id = ?2),
                             ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                             COALESCE((SELECT p.name FROM projects p JOIN sessions s ON s.project_id = p.id WHERE s.id = ?2), ''),
                             COALESCE((SELECT a.name FROM accounts a JOIN sessions s ON s.account_id = a.id WHERE s.id = ?2), ''),
                             COALESCE((SELECT a.provider FROM accounts a JOIN sessions s ON s.account_id = a.id WHERE s.id = ?2), ''),
                             COALESCE((SELECT value FROM app_settings WHERE key = 'bridge.machine_id'), ''),
                             COALESCE((SELECT value FROM app_settings WHERE key = 'bridge.machine_name'), ''))",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        session_id,
                        block_id,
                        usage.input_tokens,
                        usage.output_tokens,
                        usage.cache_read_tokens,
                        usage.cache_creation_tokens,
                        usage.cost_usd,
                        chrono::Utc::now().to_rfc3339(),
                        // See `usage_meter::record_turns`: a fact about when
                        // the turn ran, not about now.
                        crate::commands::graph::graph_available_for_session(&conn, session_id) as i64,
                    ],
                ) {
                    log::error!("store_turn_usage: failed to persist usage for session {session_id}: {e}");
                }
            }
            Err(e) => log::error!("store_turn_usage: DB mutex poisoned for session {session_id}: {e}"),
        }
    }
    check_spend_cap(app, session_id);
}

/// Fires a one-time-per-app-run notification when an account's trailing
/// 7-day recorded spend (`turn_usage.cost_usd`, Hangar's own numbers — not a
/// live provider quota) crosses its `monthly_cap / 4.345` weekly estimate.
/// Only meaningful for `billing = 'key'` accounts (subscription accounts
/// have no $ cap — see `commands::settings::set_account_monthly_cap`'s
/// callers). The dedup lives only in memory (`SpendCapNotified`, reset on
/// restart) — this is a "heads up, you're over" nudge, not an exact
/// per-billing-period counter.
/// Pure threshold check, isolated so the math is testable without a DB or
/// app handle: `billing` must be `"key"` (subscription accounts have no $
/// cap) and a `monthly_cap` must be set, in which case a week's worth of
/// recorded spend is compared against `monthly_cap / 4.345`.
fn is_over_weekly_spend_estimate(billing: &str, monthly_cap: Option<f64>, trailing_7day_cost: f64) -> bool {
    if billing != "key" {
        return false;
    }
    let Some(monthly_cap) = monthly_cap else { return false };
    let weekly_estimate = monthly_cap / 4.345;
    weekly_estimate > 0.0 && trailing_7day_cost >= weekly_estimate
}

fn check_spend_cap<R: Runtime>(app: &AppHandle<R>, session_id: &str) {
    let Some(db) = app.try_state::<DbConnection>() else { return };
    let Ok(conn) = db.0.lock() else { return };

    let row: Option<(String, String, Option<f64>, String)> = conn
        .query_row(
            "SELECT s.account_id, a.billing, a.monthly_cap, a.name FROM sessions s JOIN accounts a ON a.id = s.account_id WHERE s.id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok();
    let Some((account_id, billing, monthly_cap, account_name)) = row else { return };

    let seven_days_ago = (chrono::Utc::now() - chrono::Duration::days(7)).to_rfc3339();
    let trailing_cost: f64 = conn
        .query_row(
            // Summed off `turn_usage.account_id` directly. It used to join
            // through `sessions`, which silently excluded every turn without a
            // session row — and that is where most spending happens: measured
            // on a real database, $1,253 of $2,065 (61%) came from terminals
            // with no session, so the cap was watching a third of the money and
            // never fired.
            "SELECT COALESCE(SUM(cost_usd), 0.0) FROM turn_usage WHERE account_id = ?1 AND created_at >= ?2",
            params![account_id, seven_days_ago],
            |row| row.get(0),
        )
        .unwrap_or(0.0);
    if !is_over_weekly_spend_estimate(&billing, monthly_cap, trailing_cost) {
        return;
    }

    let Ok(settings) = crate::commands::settings::read_app_settings(&conn) else { return };
    drop(conn);
    if !settings.notify_spend_cap {
        return;
    }

    if let Some(notified) = app.try_state::<SpendCapNotified>() {
        let Ok(mut set) = notified.0.lock() else { return };
        if !set.insert(account_id) {
            return; // already notified this app run
        }
    }

    crate::notifications::notify_if_enabled(
        app,
        |s| s.notify_spend_cap,
        "Spend cap exceeded",
        &format!("{account_name} passed its estimated weekly budget (${trailing_cost:.2} spent)"),
    );
}

fn store_provider_session_id<R: Runtime>(app: &AppHandle<R>, session_id: &str, provider_session_id: &str) {
    if let Some(db) = app.try_state::<DbConnection>() {
        match db.0.lock() {
            Ok(conn) => {
                if let Err(e) = conn.execute(
                    "UPDATE sessions SET provider_session_id = ?2 WHERE id = ?1",
                    params![session_id, provider_session_id],
                ) {
                    log::error!("store_provider_session_id: failed for session {session_id}: {e}");
                }
            }
            Err(e) => log::error!("store_provider_session_id: DB mutex poisoned for session {session_id}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with_registries() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_app();
        app.manage(TurnRegistry::new());
        app.manage(CancelledTurns::new());
        app
    }

    /// The double-spawn guard: two `send_message` calls in quick succession
    /// must not both start a `claude --resume <same id>` against the same
    /// on-disk transcript (the second would also have overwritten the first's
    /// pid in the registry, orphaning a process "Stop" could no longer kill).
    #[test]
    fn a_second_turn_on_the_same_session_is_refused_until_the_first_finishes() {
        let app = app_with_registries();
        let handle = app.handle();

        assert!(try_begin_turn(handle, "sess1"), "the first turn must be allowed");
        assert!(!try_begin_turn(handle, "sess1"), "a second concurrent turn on the same session must be refused");

        // A different session is unaffected — the guard is per session, not global.
        assert!(try_begin_turn(handle, "sess2"));

        // Once the first turn finishes and releases its slot, the session is usable again.
        unregister_turn(handle, "sess1");
        assert!(try_begin_turn(handle, "sess1"), "the slot must be reusable after the turn releases it");
    }

    /// Regression for "pressing Stop undoes itself": `kill_turn` must record
    /// the cancellation so the turn's own tail skips its `waiting` transition
    /// and its "Session finished" notification, leaving `stop_session`'s
    /// `done` write standing.
    #[test]
    fn stopping_a_turn_flags_it_cancelled_exactly_once() {
        let app = app_with_registries();
        let handle = app.handle();

        assert!(try_begin_turn(handle, "sess1"));
        assert!(kill_turn(handle, "sess1"), "stopping a live turn must report that there was one");
        assert!(take_cancelled(handle, "sess1"), "the turn's tail must see the cancellation");
        assert!(
            !take_cancelled(handle, "sess1"),
            "the flag must be consumed, not left behind to suppress the *next* turn's completion"
        );
    }

    /// Stopping a session with nothing running must not leave a flag armed —
    /// otherwise the next real turn on that session would finish silently,
    /// never leaving `running` in the UI.
    #[test]
    fn stopping_an_idle_session_arms_nothing() {
        let app = app_with_registries();
        let handle = app.handle();

        assert!(!kill_turn(handle, "sess1"), "there was no turn to stop");
        assert!(!take_cancelled(handle, "sess1"));
        assert!(try_begin_turn(handle, "sess1"), "a later turn must start normally");
    }

    /// Stop pressed in the window between reserving the slot and the child
    /// actually spawning: there's no pid to signal, but it still counts as a
    /// cancellation so the turn about to start bails out.
    #[test]
    fn a_turn_cancelled_before_its_process_spawned_still_counts_as_cancelled() {
        let app = app_with_registries();
        let handle = app.handle();

        try_begin_turn(handle, "sess1");
        // No `register_turn` call — the registry still holds `None` for this session.
        assert!(kill_turn(handle, "sess1"));
        assert!(take_cancelled(handle, "sess1"));
    }

    #[test]
    fn is_over_weekly_spend_estimate_ignores_subscription_accounts() {
        assert!(!is_over_weekly_spend_estimate("sub", Some(10.0), 1_000_000.0), "subscription accounts have no $ cap");
    }

    #[test]
    fn is_over_weekly_spend_estimate_ignores_accounts_with_no_cap_set() {
        assert!(!is_over_weekly_spend_estimate("key", None, 1_000_000.0));
    }

    #[test]
    fn is_over_weekly_spend_estimate_compares_against_monthly_cap_over_4_345() {
        // monthly_cap 100 -> weekly estimate ~23.015. Under, exactly at, and over.
        let weekly_estimate = 100.0 / 4.345;
        assert!(!is_over_weekly_spend_estimate("key", Some(100.0), 22.0));
        assert!(is_over_weekly_spend_estimate("key", Some(100.0), weekly_estimate));
        assert!(is_over_weekly_spend_estimate("key", Some(100.0), 50.0));
    }

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
        // "Fleet Agent" is what the recorded fixture literally says — it was
        // captured from a real turn back when the project had that name. The
        // fixture is evidence, not prose: it stays verbatim, so this assertion
        // must keep matching it rather than being renamed along with the app.
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
    fn capture_file_edits_extracts_old_and_new_text_for_edit() {
        let input = serde_json::json!({"file_path": "src/main.rs", "old_string": "let a = 1;", "new_string": "let a = 2;"});
        let edits = capture_file_edits("Edit", &input);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].file_path, "src/main.rs");
        assert_eq!(edits[0].tool_name, "Edit");
        assert_eq!(edits[0].old_text.as_deref(), Some("let a = 1;"));
        assert_eq!(edits[0].new_text.as_deref(), Some("let a = 2;"));
    }

    #[test]
    fn capture_file_edits_for_write_has_no_old_text() {
        let input = serde_json::json!({"file_path": "src/new.rs", "content": "fn main() {}"});
        let edits = capture_file_edits("Write", &input);
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].old_text, None, "a freshly written file has no prior content");
        assert_eq!(edits[0].new_text.as_deref(), Some("fn main() {}"));
    }

    #[test]
    fn capture_file_edits_for_multi_edit_yields_one_capture_per_replacement() {
        let input = serde_json::json!({
            "file_path": "src/lib.rs",
            "edits": [
                {"old_string": "foo", "new_string": "bar"},
                {"old_string": "baz", "new_string": "qux"},
            ],
        });
        let edits = capture_file_edits("MultiEdit", &input);
        assert_eq!(edits.len(), 2);
        assert!(edits.iter().all(|e| e.file_path == "src/lib.rs" && e.tool_name == "MultiEdit"));
        assert_eq!(edits[0].old_text.as_deref(), Some("foo"));
        assert_eq!(edits[1].new_text.as_deref(), Some("qux"));
    }

    #[test]
    fn capture_file_edits_ignores_unrelated_tools_and_missing_file_path() {
        assert!(capture_file_edits("Bash", &serde_json::json!({"command": "ls"})).is_empty());
        assert!(capture_file_edits("Edit", &serde_json::json!({"old_string": "a", "new_string": "b"})).is_empty());
    }

    #[test]
    fn parse_line_surfaces_file_edits_from_an_assistant_message() {
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "tool_use", "name": "Edit", "input": {"file_path": "a.txt", "old_string": "x", "new_string": "y"}}
                ]
            }
        })
        .to_string();
        let outcome = parse_line(&line, "claude");
        assert_eq!(outcome.file_edits.len(), 1);
        assert_eq!(outcome.file_edits[0].file_path, "a.txt");
        assert_eq!(
            outcome.file_edits[0].source_block_index,
            Some(0),
            "the Edit tool_use item also pushes exactly one 'edit' block, at index 0 — the capture must point back to it"
        );
    }

    #[test]
    fn multi_edit_captures_all_point_at_the_same_source_block() {
        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "tool_use", "name": "MultiEdit", "input": {
                        "file_path": "a.txt",
                        "edits": [
                            {"old_string": "x1", "new_string": "y1"},
                            {"old_string": "x2", "new_string": "y2"},
                        ]
                    }}
                ]
            }
        })
        .to_string();
        let outcome = parse_line(&line, "claude");
        assert_eq!(outcome.blocks.len(), 1, "MultiEdit produces a single summary block");
        assert_eq!(outcome.file_edits.len(), 2);
        assert!(outcome.file_edits.iter().all(|e| e.source_block_index == Some(0)));
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
    ///
    /// `#[ignore]` because it spawns the real CLI: worth running locally
    /// (`cargo test -- --ignored`), but on CI it would only ever assert that
    /// the runner has no `claude` installed.
    #[test]
    #[ignore = "spawns a real `claude` process; requires the CLI installed"]
    fn spawn_turn_writes_you_block_and_final_result_even_when_not_logged_in() {
        use rusqlite::Connection;
        use std::sync::Mutex as StdMutex;
        use std::time::{Duration, Instant};

        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();

        let isolated_config_dir = std::env::temp_dir().join(format!("hangar-test-turn-{}", uuid::Uuid::new_v4()));
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
            instructions: None,
            mcp_config: None,
        })
        .expect("the first turn on a fresh session must not be refused");

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

    /// `apply_line` must thread the real persisted block id through to
    /// `file_edits.block_id` — regression test for the bug where it was
    /// always inserted as NULL because `emit_block` generated its uuid
    /// internally and discarded it.
    #[test]
    fn apply_line_links_file_edits_to_their_source_block() {
        use rusqlite::Connection;
        use std::sync::Mutex as StdMutex;

        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('sess1','test-session','proj1','acc1','','high','ask_edits','waiting','now')",
            [],
        ).unwrap();
        app.manage(crate::db::DbConnection(StdMutex::new(conn)));

        let line = serde_json::json!({
            "type": "assistant",
            "message": {
                "content": [
                    {"type": "tool_use", "name": "Edit", "input": {"file_path": "a.txt", "old_string": "x", "new_string": "y"}}
                ]
            }
        })
        .to_string();
        apply_line(&app.handle().clone(), "sess1", "acc1", &line, &mut None);

        let db = app.state::<crate::db::DbConnection>();
        let conn = db.0.lock().unwrap();
        let block_id: String = conn
            .query_row("SELECT id FROM session_blocks WHERE session_id = 'sess1' AND kind = 'edit'", [], |r| r.get(0))
            .expect("apply_line must persist the 'edit' summary block");
        let edit_block_id: Option<String> = conn
            .query_row("SELECT block_id FROM file_edits WHERE session_id = 'sess1'", [], |r| r.get(0))
            .expect("apply_line must persist the file_edits row");
        assert_eq!(edit_block_id, Some(block_id), "file_edits.block_id must point at the real 'edit' block, not NULL");
    }

    #[test]
    fn parse_line_captures_usage_from_result_message() {
        let line = serde_json::json!({
            "type": "result",
            "result": "Done.",
            "total_cost_usd": 0.0123,
            "usage": {
                "input_tokens": 150,
                "output_tokens": 70,
                "cache_read_input_tokens": 40,
                "cache_creation_input_tokens": 10,
            }
        })
        .to_string();
        let outcome = parse_line(&line, "claude");
        assert_eq!(outcome.blocks.len(), 1, "the result text produces a single 'agent' block");
        let usage = outcome.turn_usage.expect("usage must be captured from the result message");
        assert_eq!(usage.input_tokens, 150);
        assert_eq!(usage.output_tokens, 70);
        assert_eq!(usage.cache_read_tokens, Some(40));
        assert_eq!(usage.cache_creation_tokens, Some(10));
        assert_eq!(usage.cost_usd, Some(0.0123));
        assert_eq!(usage.source_block_index, Some(0), "usage must point at the 'agent' block from the same result message");
    }

    /// A turn that aborts/errors can end with `{"type":"result","result":""}`
    /// — this must NOT produce a blank 'agent' block, both because it's
    /// misleading (looks like a real, if silent, response) and because a
    /// non-empty `blocks` count would suppress `spawn_turn`'s "never finish
    /// in total silence" backstop, which needs `stdout_blocks == 0` to fire.
    #[test]
    fn parse_line_ignores_empty_result_text() {
        let line = serde_json::json!({ "type": "result", "result": "" }).to_string();
        let outcome = parse_line(&line, "claude");
        assert_eq!(outcome.blocks.len(), 0, "an empty result string must not produce an 'agent' block");
    }

    #[test]
    fn apply_line_links_turn_usage_to_its_source_block() {
        use rusqlite::Connection;
        use std::sync::Mutex as StdMutex;

        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('sess1','test-session','proj1','acc1','','high','ask_edits','waiting','now')",
            [],
        ).unwrap();
        app.manage(crate::db::DbConnection(StdMutex::new(conn)));

        let line = serde_json::json!({
            "type": "result",
            "result": "Done.",
            "total_cost_usd": 0.05,
            "usage": {"input_tokens": 200, "output_tokens": 80}
        })
        .to_string();
        apply_line(&app.handle().clone(), "sess1", "acc1", &line, &mut None);

        let db = app.state::<crate::db::DbConnection>();
        let conn = db.0.lock().unwrap();
        let block_id: String = conn
            .query_row("SELECT id FROM session_blocks WHERE session_id = 'sess1' AND kind = 'agent'", [], |r| r.get(0))
            .expect("apply_line must persist the 'agent' block");
        let usage_block_id: Option<String> = conn
            .query_row("SELECT block_id FROM turn_usage WHERE session_id = 'sess1'", [], |r| r.get(0))
            .expect("apply_line must persist the turn_usage row");
        assert_eq!(usage_block_id, Some(block_id), "turn_usage.block_id must point at the real 'agent' block, not NULL");
    }

    /// The exact bug reported live: Claude Code's terminal `result` message
    /// echoes the same text as the assistant's own last response verbatim
    /// (confirmed against `tests/fixtures/real_turn_simple.jsonl`) — without
    /// the dedup in `apply_line`, this produced two identical AGENT blocks
    /// in the transcript for a single actual response.
    #[test]
    fn apply_line_skips_duplicate_agent_block_when_result_echoes_last_assistant_text() {
        use rusqlite::Connection;
        use std::sync::Mutex as StdMutex;

        let app = tauri::test::mock_app();
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp','now')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('sess1','test-session','proj1','acc1','','high','ask_edits','waiting','now')",
            [],
        ).unwrap();
        app.manage(crate::db::DbConnection(StdMutex::new(conn)));

        let assistant_line = serde_json::json!({
            "type": "assistant",
            "message": { "content": [{ "type": "text", "text": "Same text." }] }
        })
        .to_string();
        let result_line = serde_json::json!({ "type": "result", "result": "Same text." }).to_string();

        let mut last_agent_block: Option<(String, String)> = None;
        apply_line(&app.handle().clone(), "sess1", "acc1", &assistant_line, &mut last_agent_block);
        apply_line(&app.handle().clone(), "sess1", "acc1", &result_line, &mut last_agent_block);

        let db = app.state::<crate::db::DbConnection>();
        let conn = db.0.lock().unwrap();
        let agent_block_count: i64 = conn.query_row("SELECT COUNT(*) FROM session_blocks WHERE session_id = 'sess1' AND kind = 'agent'", [], |r| r.get(0)).unwrap();
        assert_eq!(agent_block_count, 1, "the result's echoed text must not create a second 'agent' block");
    }
}
