//! One question to a model, one structured answer back — for the small jobs
//! Hangar asks an AI to do on its own (sorting notifications), as opposed to
//! the sessions and terminals where *you* talk to an agent.
//!
//! Runs `claude -p` under the account the user picked, so it works for every
//! account that runs Claude Code (Anthropic, Kimi, custom endpoints) with that
//! account's own login, key and endpoint — the same environment its terminal
//! gets. `--json-schema` makes the CLI validate the answer's shape, and the
//! spend lands in Usage like any other turn.
//!
//! No tools, no MCP servers, no saved session, and a working directory of its
//! own: the prompt is all the model gets to see, and nothing it says can act.

use crate::db::DbConnection;
use crate::providers::claude;
use crate::usage_meter::TurnCost;
use serde_json::Value;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};

/// Long enough for a slow first start (a cold `claude` plus a model that
/// thinks), short enough that a hung CLI doesn't leave a button spinning.
const TIMEOUT: Duration = Duration::from_secs(90);

/// The command line for one structured question. `model` only for accounts
/// whose model is Anthropic's to name — a Kimi or custom account has its own
/// model set by its endpoint, and overriding it would send a model id the
/// server doesn't know.
pub(crate) fn oneshot_args(prompt: &str, schema: &str, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        prompt.into(),
        "--output-format".into(),
        "json".into(),
        "--json-schema".into(),
        schema.into(),
        // `""` is the CLI's own spelling for "no tools at all".
        "--tools".into(),
        "".into(),
        "--strict-mcp-config".into(),
        "--no-session-persistence".into(),
    ];
    if let Some(model) = model {
        args.push("--model".into());
        args.push(model.into());
    }
    args
}

/// What the CLI printed, reduced to the answer and what it cost.
pub(crate) fn parse_output(stdout: &str) -> Result<(Value, TurnCost), String> {
    let out: Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        let first = stdout.lines().find(|l| !l.trim().is_empty()).unwrap_or("no output");
        format!("claude answered something that isn't JSON: {first}")
    })?;
    let result_text = out.get("result").and_then(Value::as_str).unwrap_or("");
    if out.get("is_error").and_then(Value::as_bool).unwrap_or(false) {
        if claude::is_exhaustion_signal(result_text) {
            return Err(format!("this account is out of quota right now — pick another one ({result_text})"));
        }
        return Err(format!("claude failed: {}", if result_text.is_empty() { "no reason given" } else { result_text }));
    }
    // `structured_output` is where `--json-schema` puts the validated answer;
    // the text result is the same JSON, and a fallback for a CLI that only
    // filled that.
    let answer = match out.get("structured_output") {
        Some(v) if !v.is_null() => v.clone(),
        _ => serde_json::from_str(result_text).map_err(|_| "claude didn't return the structured answer it was asked for".to_string())?,
    };

    let usage = out.get("usage");
    let n = |key: &str| usage.and_then(|u| u.get(key)).and_then(Value::as_i64).unwrap_or(0);
    let cost = TurnCost {
        message_id: out
            .get("uuid")
            .or_else(|| out.get("session_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        model: out.get("modelUsage").and_then(Value::as_object).and_then(|m| m.keys().next().cloned()),
        input_tokens: n("input_tokens"),
        output_tokens: n("output_tokens"),
        cache_read_tokens: n("cache_read_input_tokens"),
        cache_creation_tokens: n("cache_creation_input_tokens"),
        at: Some(chrono::Utc::now().to_rfc3339()),
    };
    Ok((answer, cost))
}

/// Asks `prompt` under `account_id` and returns the answer, shaped by `schema`.
/// Blocking — call it from `run_blocking`.
pub(crate) fn ask<R: Runtime>(app: &AppHandle<R>, account_id: &str, prompt: &str, schema: &Value) -> Result<Value, String> {
    let db = app.state::<DbConnection>();
    let account = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::terminals::load_account(&conn, account_id)?
    };
    let spec = crate::providers::spec(&account.provider).ok_or_else(|| format!("unknown provider '{}'", account.provider))?;
    if spec.binary != claude::BINARY {
        return Err(format!("{} runs {}, and only accounts that run Claude Code can do this yet", account.name, spec.binary));
    }

    let mut env = vec![(spec.config_dir_env.to_string(), account.config_dir_path.clone())];
    env.extend(crate::terminals::account_key_env(&account, account_id)?);

    // Its own empty folder, so no project's CLAUDE.md or settings ride along.
    let cwd = crate::app_data::root(app)?.join("ai-oneshot");
    std::fs::create_dir_all(&cwd).map_err(|e| format!("could not prepare a working folder: {e}"))?;

    let model = (account.provider == "claude").then_some("haiku");
    let mut cmd = std::process::Command::new(claude::binary());
    cmd.args(oneshot_args(prompt, &schema.to_string(), model)).current_dir(&cwd);
    for (k, v) in &env {
        cmd.env(k, v);
    }

    let output = crate::process_timeout::run_with_timeout(cmd, TIMEOUT)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() && stdout.trim().is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("claude exited without an answer: {}", stderr.lines().last().unwrap_or("no message")));
    }
    let (answer, cost) = parse_output(&stdout)?;
    crate::usage_meter::record_turns(&db, account_id, None, None, &[cost]);
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `claude 2.1.302 -p … --output-format json --json-schema …`.
    const REAL: &str = r#"{"type":"result","subtype":"success","is_error":false,"session_id":"60fddf8f","uuid":"c5db6551","total_cost_usd":0.0010026,
      "usage":{"input_tokens":2,"cache_creation_input_tokens":4297,"cache_read_input_tokens":0,"output_tokens":286},
      "modelUsage":{"claude-haiku-5-5":{"inputTokens":2,"outputTokens":286}},
      "result":"{\"items\":[{\"id\":\"a\",\"category\":\"action_required\",\"priority\":1,\"reason\":\"blocked\"}]}",
      "structured_output":{"items":[{"id":"a","category":"action_required","priority":1,"reason":"blocked"}]}}"#;

    #[test]
    fn the_structured_answer_and_its_cost_are_read() {
        let (answer, cost) = parse_output(REAL).unwrap();
        assert_eq!(answer["items"][0]["id"], "a");
        assert_eq!(cost.message_id, "c5db6551");
        assert_eq!(cost.model.as_deref(), Some("claude-haiku-5-5"));
        assert_eq!(cost.output_tokens, 286);
        assert_eq!(cost.cache_creation_tokens, 4297);
    }

    #[test]
    fn the_text_result_is_a_fallback() {
        let raw = r#"{"type":"result","is_error":false,"result":"{\"items\":[]}"}"#;
        assert_eq!(parse_output(raw).unwrap().0["items"], serde_json::json!([]));
    }

    #[test]
    fn an_error_result_says_why() {
        let raw = r#"{"type":"result","is_error":true,"result":"Claude AI usage limit reached"}"#;
        assert!(parse_output(raw).unwrap_err().contains("out of quota"));
        assert!(parse_output("Error: not logged in").unwrap_err().contains("isn't JSON"));
    }

    #[test]
    fn no_tools_and_no_saved_session() {
        let args = oneshot_args("q", "{}", None);
        let tools = args.iter().position(|a| a == "--tools").unwrap();
        assert_eq!(args[tools + 1], "");
        assert!(args.contains(&"--no-session-persistence".to_string()));
        assert!(!args.contains(&"--model".to_string()));
        assert!(oneshot_args("q", "{}", Some("haiku")).ends_with(&["--model".to_string(), "haiku".to_string()]));
    }
}
