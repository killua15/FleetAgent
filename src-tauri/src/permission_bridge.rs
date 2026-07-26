//! Bridges Claude Code's own stdio control protocol (used internally by its
//! Agent SDK's `canUseTool` and the VS Code extension) to Hangar's UI.
//!
//! Claude Code can't prompt for permission interactively in headless (`-p`)
//! mode — there's no TTY — but `--permission-prompt-tool stdio` (combined
//! with `--input-format stream-json`) makes it emit a `control_request` JSON
//! line on its own stdout and then block reading its own stdin for a
//! matching `control_response` line. Since Hangar already owns that child
//! process's stdin/stdout (`turn_runner::spawn_turn`), no separate hook
//! process or local server is needed: `parse_control_request` and
//! `build_control_response` here are pure text<->struct helpers, and
//! `await_decision` is the only stateful piece — it emits the same
//! `permission-request` event the UI already listens for and blocks until
//! `respond_to_permission_request` (wired to the Approve/Deny buttons in
//! AgentPanel) answers it.
//!
//! Verified against a real `claude` 2.1.212 install: the CLI has no timeout
//! of its own waiting on stdin for the response — a `control_request` left
//! unanswered for 45s had zero effect — so `APPROVAL_TIMEOUT` below is a
//! safety net against an abandoned session, not a race against anything.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, Runtime};

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub struct PendingApprovals(pub Mutex<HashMap<String, mpsc::Sender<bool>>>);

impl PendingApprovals {
    pub fn new() -> Self {
        PendingApprovals(Mutex::new(HashMap::new()))
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PermissionRequestEvent {
    request_id: String,
    session_id: String,
    tool_name: String,
    tool_input: Value,
}

/// One `can_use_tool` control request parsed off a `claude --output-format
/// stream-json` stdout line.
#[derive(Debug, Clone, PartialEq)]
pub struct ControlRequest {
    pub request_id: String,
    pub tool_name: String,
    pub tool_input: Value,
}

/// Parses a stdout line as a `control_request`/`can_use_tool` message.
/// Returns `None` for every other line (session blocks, rate-limit events,
/// plain text, ...) — the caller falls through to the normal block parser
/// in that case.
pub fn parse_control_request(line: &str) -> Option<ControlRequest> {
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("type").and_then(|v| v.as_str()) != Some("control_request") {
        return None;
    }
    let request = value.get("request")?;
    if request.get("subtype").and_then(|v| v.as_str()) != Some("can_use_tool") {
        return None;
    }
    let request_id = value.get("request_id").and_then(|v| v.as_str())?.to_string();
    let tool_name = request.get("tool_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let tool_input = request.get("input").cloned().unwrap_or(Value::Null);
    Some(ControlRequest { request_id, tool_name, tool_input })
}

/// Builds the `control_response` line to write back to the child's stdin.
pub fn build_control_response(request_id: &str, allow: bool) -> String {
    let response = if allow {
        json!({ "behavior": "allow" })
    } else {
        json!({ "behavior": "deny", "message": "denied by user in Hangar" })
    };
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": response,
        },
    })
    .to_string()
}

/// Registers `req` as pending, emits `permission-request` to the UI, and
/// blocks until `respond_to_permission_request` answers it (or the safety-net
/// timeout elapses, which denies). Called from the turn's own stdout-reading
/// thread in `turn_runner::spawn_turn`, right after `parse_control_request`
/// recognizes a line — that thread already owns the child's stdin, so it
/// writes the resulting `control_response` itself once this returns.
pub fn await_decision<R: Runtime>(app: &AppHandle<R>, session_id: &str, req: &ControlRequest) -> bool {
    let (tx, rx) = mpsc::channel::<bool>();

    if let Some(state) = app.try_state::<PendingApprovals>() {
        if let Ok(mut map) = state.0.lock() {
            map.insert(req.request_id.clone(), tx);
        }
    }

    let _ = app.emit(
        "permission-request",
        PermissionRequestEvent {
            request_id: req.request_id.clone(),
            session_id: session_id.to_string(),
            tool_name: req.tool_name.clone(),
            tool_input: req.tool_input.clone(),
        },
    );
    crate::notifications::notify_if_enabled(
        app,
        |s| s.notify_needs_approval,
        "Hangar needs your approval",
        &format!("{} wants to run {} — click to review", session_id, req.tool_name),
    );

    let allow = rx.recv_timeout(APPROVAL_TIMEOUT).unwrap_or(false);

    if let Some(state) = app.try_state::<PendingApprovals>() {
        if let Ok(mut map) = state.0.lock() {
            map.remove(&req.request_id);
        }
    }

    allow
}

/// Called by the Approve/Deny buttons in AgentPanel's permission banner.
#[tauri::command]
pub fn respond_to_permission_request(app: AppHandle, request_id: String, allow: bool) -> Result<(), String> {
    respond_to_permission_request_impl(&app, &request_id, allow)
}

/// Generic over `Runtime` so tests can drive it with `tauri::test::mock_app()`
/// instead of only the real Wry-backed app (same pattern used elsewhere in
/// this codebase, e.g. `turn_runner::kill_turn`).
fn respond_to_permission_request_impl<R: Runtime>(app: &AppHandle<R>, request_id: &str, allow: bool) -> Result<(), String> {
    let state = app.state::<PendingApprovals>();
    let mut map = state.0.lock().map_err(|e| e.to_string())?;
    match map.remove(request_id) {
        Some(sender) => {
            let _ = sender.send(allow);
            Ok(())
        }
        None => Err("no pending approval with that id — it may have already timed out".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_control_request_extracts_tool_name_and_input() {
        let line = r#"{"type":"control_request","request_id":"abc-123","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"touch /tmp/x.txt"},"tool_use_id":"toolu_1"}}"#;
        let parsed = parse_control_request(line).expect("must parse a real control_request line");
        assert_eq!(parsed.request_id, "abc-123");
        assert_eq!(parsed.tool_name, "Bash");
        assert_eq!(parsed.tool_input["command"], "touch /tmp/x.txt");
    }

    #[test]
    fn parse_control_request_ignores_other_message_types() {
        assert_eq!(parse_control_request(r#"{"type":"system","subtype":"init"}"#), None);
        assert_eq!(parse_control_request(r#"{"type":"assistant","message":{}}"#), None);
        assert_eq!(parse_control_request("not json at all"), None);
    }

    #[test]
    fn parse_control_request_ignores_non_can_use_tool_control_requests() {
        let line = r#"{"type":"control_request","request_id":"x","request":{"subtype":"something_else"}}"#;
        assert_eq!(parse_control_request(line), None);
    }

    #[test]
    fn build_control_response_allow_shape() {
        let response = build_control_response("abc-123", true);
        let parsed: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["type"], "control_response");
        assert_eq!(parsed["response"]["request_id"], "abc-123");
        assert_eq!(parsed["response"]["response"]["behavior"], "allow");
    }

    #[test]
    fn build_control_response_deny_shape_includes_a_message() {
        let response = build_control_response("abc-123", false);
        let parsed: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(parsed["response"]["response"]["behavior"], "deny");
        assert!(parsed["response"]["response"]["message"].as_str().unwrap().contains("Hangar"));
    }

    #[test]
    fn await_decision_unblocks_when_respond_to_permission_request_is_called() {
        use tauri::Listener;

        let app = tauri::test::mock_app();
        app.manage(PendingApprovals::new());

        let handle = app.handle().clone();
        let req = ControlRequest { request_id: "req-1".to_string(), tool_name: "Bash".to_string(), tool_input: json!({"command": "echo hi"}) };

        let captured_event: std::sync::Arc<Mutex<Option<Value>>> = std::sync::Arc::new(Mutex::new(None));
        let captured_clone = captured_event.clone();
        app.listen("permission-request", move |event| {
            *captured_clone.lock().unwrap() = serde_json::from_str(event.payload()).ok();
        });

        let worker = std::thread::spawn(move || await_decision(&handle, "session-1", &req));

        // Wait for the event to fire before answering, same as a real UI
        // click would only happen after seeing the banner.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while captured_event.lock().unwrap().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(captured_event.lock().unwrap().is_some(), "permission-request event must fire");

        respond_to_permission_request_impl(app.handle(), "req-1", true).expect("must resolve a real pending request");
        assert!(worker.join().unwrap());
    }
}
