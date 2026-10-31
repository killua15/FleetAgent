//! Running one agent of the line: a headless turn of Claude Code or Codex
//! under the agent's account and model, in the order's worktree, answering in
//! a JSON shape. What it does on the way (reads, edits, commands) comes out as
//! activity for the live feed; a usage limit or a lack of credits comes out as
//! such, never as a failure.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::limits::{self, Signal};

/// Everything one agent turn needs.
#[derive(Debug, Clone)]
pub struct AgentCall {
    /// The project it works for: ⏹ on the project stops it.
    pub project: String,
    /// What it is within the project (the run, or `plan`), to cancel just it.
    pub key: String,
    pub account_id: String,
    pub model: String,
    pub cwd: PathBuf,
    pub prompt: String,
    pub schema: Value,
    /// Continue this conversation instead of starting one — after a limit,
    /// or for another round in Fabricación.
    pub resume: Option<String>,
    /// MCP servers for this agent only (the Chrome tool, for UI/UX).
    pub mcp_config: Option<PathBuf>,
    /// May edit files and run commands in `cwd` (builder, testers); otherwise
    /// it only reads.
    pub write: bool,
    pub timeout: Duration,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Ok,
    /// The account ran out of its window; go on with `resume` after `resume_at`.
    Limit { resume_at: DateTime<Utc> },
    NoCredits,
    Stopped,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct AgentResult {
    pub outcome: Outcome,
    pub json: Option<Value>,
    /// The CLI's conversation id, to resume it.
    pub session_id: Option<String>,
    pub cost_usd: f64,
}

/// One thing an agent did, for the feed: `read`, `edit`, `cmd` or `text`.
#[derive(Debug, Clone, PartialEq)]
pub struct Activity {
    pub kind: &'static str,
    pub text: String,
}

/// Runs agents. The line takes it as a trait so its tests can script every
/// answer without starting a CLI.
pub trait Runner: Send + Sync {
    fn run(&self, call: &AgentCall, on: &mut dyn FnMut(Activity)) -> AgentResult;
}

/// The account's CLI, binary and environment.
#[derive(Debug, Clone)]
pub struct Launch {
    pub provider: String,
    pub binary: String,
    pub env: Vec<(String, String)>,
}

/// The real runner: the account's own CLI.
pub struct CliRunner {
    /// Account id → how to launch it, resolved once per order by the line.
    pub launches: HashMap<String, Launch>,
    /// App data root, where Claude Code's status line leaves each account's
    /// rate-limit resets.
    pub root: Option<PathBuf>,
}

/// The rows of the accounts a team uses — read with the database lock held,
/// so that the slow part (binaries, the keychain) can run without it.
pub fn accounts_for(conn: &rusqlite::Connection, account_ids: &[String]) -> Result<Vec<(String, crate::terminals::AccountForTerminal)>, String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in account_ids {
        if seen.insert(id.clone()) {
            out.push((id.clone(), crate::terminals::load_account(conn, id)?));
        }
    }
    Ok(out)
}

impl CliRunner {
    /// How to launch each account's CLI: its binary and its environment,
    /// a key from the keychain included. Never call it holding the database
    /// lock — a keychain prompt or a login shell can take its time.
    /// `HANGAR_FACTORY_AGENT_BIN` replaces the CLI with a fake one, to walk
    /// the line without spending.
    pub fn from_accounts(accounts: Vec<(String, crate::terminals::AccountForTerminal)>, root: Option<PathBuf>) -> Result<Self, String> {
        let fake = std::env::var("HANGAR_FACTORY_AGENT_BIN").ok();
        let mut launches = HashMap::new();
        for (id, account) in accounts {
            let spec = crate::providers::spec(&account.provider).ok_or_else(|| format!("proveedor desconocido `{}`", account.provider))?;
            if spec.binary != "codex" && spec.binary != crate::providers::claude::BINARY {
                return Err(format!("{}: la Factory todavía solo corre Claude Code y Codex", account.name));
            }
            let mut env = vec![(spec.config_dir_env.to_string(), account.config_dir_path.clone())];
            env.extend(spec.extra_env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
            env.extend(crate::terminals::account_key_env(&account, &id)?);
            let binary = match &fake {
                Some(path) => path.clone(),
                None if spec.binary == crate::providers::claude::BINARY => crate::providers::claude::binary(),
                None => crate::providers::resolve_binary(spec.binary),
            };
            let provider = if spec.binary == "codex" { "codex" } else { "claude" }.to_string();
            launches.insert(id, Launch { provider, binary, env });
        }
        Ok(CliRunner { launches, root })
    }

    /// Both phases, locking the database only for the first.
    pub fn load(db: &crate::db::DbConnection, account_ids: &[String], root: Option<PathBuf>) -> Result<Self, String> {
        let accounts = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            accounts_for(&conn, account_ids)?
        };
        Self::from_accounts(accounts, root)
    }
}

/// Claude Code's arguments for one agent turn.
pub fn claude_args(call: &AgentCall) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        call.prompt.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--model".into(),
        call.model.clone(),
        "--json-schema".into(),
        call.schema.to_string(),
    ];
    if call.write {
        // Its own worktree: edits and commands are what it's there for.
        args.extend(["--permission-mode".into(), "acceptEdits".into(), "--allowedTools".into(), "Bash Read Edit Write Glob Grep".into()]);
    } else {
        args.extend(["--allowedTools".into(), "Read Glob Grep Bash(git diff:*) Bash(git log:*) Bash(git show:*)".into(), "--disallowedTools".into(), "Edit Write".into()]);
    }
    if let Some(mcp) = &call.mcp_config {
        args.extend(["--mcp-config".into(), mcp.to_string_lossy().to_string()]);
    }
    if let Some(id) = &call.resume {
        args.extend(["--resume".into(), id.clone()]);
    }
    args
}

/// Codex's arguments: `codex exec` answering in the schema, its last message
/// written where we read it.
pub fn codex_args(call: &AgentCall, schema_file: &std::path::Path, last_message: &std::path::Path) -> Vec<String> {
    let mut args: Vec<String> = vec!["exec".into()];
    if let Some(id) = &call.resume {
        args.extend(["resume".into(), id.clone()]);
    }
    // `exec resume` takes neither --sandbox nor --cd: the sandbox goes as
    // config, which both accept, and the folder is the process's own.
    let sandbox = if call.write { "workspace-write" } else { "read-only" };
    args.extend([
        "--json".into(),
        "--model".into(),
        call.model.clone(),
        "--output-schema".into(),
        schema_file.to_string_lossy().to_string(),
        "--output-last-message".into(),
        last_message.to_string_lossy().to_string(),
        "-c".into(),
        format!("sandbox_mode=\"{sandbox}\""),
        "--skip-git-repo-check".into(),
    ]);
    if call.resume.is_none() {
        args.extend(["--cd".into(), call.cwd.to_string_lossy().to_string()]);
    }
    args.push(call.prompt.clone());
    args
}

/// A JSON object out of an answer that may wrap it in prose or a fence.
pub fn json_in(text: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(text.trim()) {
        if v.is_object() {
            return Some(v);
        }
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    serde_json::from_str(&text[start..=end]).ok().filter(Value::is_object)
}

/// What one line of Claude Code's stream-json says the agent did.
pub fn claude_activity(event: &Value) -> Vec<Activity> {
    let mut out = Vec::new();
    if event.get("type").and_then(Value::as_str) != Some("assistant") {
        return out;
    }
    let Some(blocks) = event.pointer("/message/content").and_then(Value::as_array) else { return out };
    for b in blocks {
        if b.get("type").and_then(Value::as_str) != Some("tool_use") {
            continue;
        }
        let name = b.get("name").and_then(Value::as_str).unwrap_or("");
        let input = b.get("input").cloned().unwrap_or(Value::Null);
        let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let (kind, text) = match name {
            "Read" => ("read", s("file_path")),
            "Glob" | "Grep" => ("read", format!("{} {}", name.to_lowercase(), s("pattern"))),
            "Edit" | "Write" | "MultiEdit" => ("edit", s("file_path")),
            "Bash" => ("cmd", s("command")),
            other => ("cmd", other.to_string()),
        };
        if !text.trim().is_empty() {
            out.push(Activity { kind, text: text.chars().take(240).collect() });
        }
    }
    out
}

impl Runner for CliRunner {
    fn run(&self, call: &AgentCall, on: &mut dyn FnMut(Activity)) -> AgentResult {
        let failed = |why: String| AgentResult { outcome: Outcome::Failed(why), json: None, session_id: None, cost_usd: 0.0 };
        let Some(launch) = self.launches.get(&call.account_id) else { return failed(format!("cuenta {} sin resolver", call.account_id)) };
        let stopped = || super::procs::job_stopped(&call.project, &call.key);
        if stopped() {
            return AgentResult { outcome: Outcome::Stopped, json: None, session_id: None, cost_usd: 0.0 };
        }

        let scratch = std::env::temp_dir().join(format!("hangar-factory-{}", uuid::Uuid::new_v4().simple()));
        let (args, last_message) = if launch.provider == "codex" {
            if std::fs::create_dir_all(&scratch).is_err() {
                return failed("no se pudo preparar la carpeta temporal".into());
            }
            let schema_file = scratch.join("schema.json");
            let last = scratch.join("last.txt");
            let _ = std::fs::write(&schema_file, call.schema.to_string());
            (codex_args(call, &schema_file, &last), Some(last))
        } else {
            (claude_args(call), None)
        };

        let mut cmd = Command::new(&launch.binary);
        cmd.args(&args).current_dir(&call.cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // The user's PATH, so the agent's own Bash finds pnpm, node, pytest.
        cmd.env("PATH", super::env::user_path());
        for (k, v) in &launch.env {
            cmd.env(k, v);
        }
        // Its own process group: ⏹ takes down the CLI and everything it ran.
        let mut tracked = match super::procs::spawn(&call.project, &call.key, &mut cmd) {
            Ok(t) => t,
            Err(_) if stopped() => return AgentResult { outcome: Outcome::Stopped, json: None, session_id: None, cost_usd: 0.0 },
            Err(e) => return failed(format!("no se pudo arrancar {}: {e}", launch.binary)),
        };
        let pid = tracked.id();
        let child = &mut tracked.child;

        // The watchdog: a turn past its timeout is killed like a ⏹.
        let done = Arc::new(Mutex::new(false));
        {
            let done = done.clone();
            let timeout = call.timeout;
            std::thread::spawn(move || {
                let step = Duration::from_secs(1);
                let mut waited = Duration::ZERO;
                while waited < timeout {
                    std::thread::sleep(step);
                    waited += step;
                    if *done.lock().unwrap_or_else(|e| e.into_inner()) {
                        return;
                    }
                }
                super::procs::kill_group(pid);
            });
        }

        let mut stderr = child.stderr.take();
        let err_reader = std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(e) = stderr.as_mut() {
                let _ = e.read_to_string(&mut s);
            }
            s
        });

        let mut session_id = None;
        let mut cost = 0.0;
        let mut result: Option<Value> = None;
        let mut final_text = String::new();
        let mut all = String::new();
        let mut saw_result = false;
        let mut result_error = false;
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                all.push_str(&line);
                all.push('\n');
                let Ok(event) = serde_json::from_str::<Value>(&line) else { continue };
                if launch.provider == "codex" {
                    if let Some(id) = event.get("thread_id").and_then(Value::as_str) {
                        session_id = Some(id.to_string());
                    }
                    if let Some(item) = event.get("item") {
                        match item.get("type").and_then(Value::as_str) {
                            Some("command_execution") => on(Activity { kind: "cmd", text: item.get("command").and_then(Value::as_str).unwrap_or("").chars().take(240).collect() }),
                            Some("file_change") => on(Activity { kind: "edit", text: item.to_string().chars().take(240).collect() }),
                            Some("agent_message") => {
                                saw_result = true;
                                final_text = item.get("text").and_then(Value::as_str).unwrap_or("").to_string();
                            }
                            _ => {}
                        }
                    }
                    continue;
                }
                if let Some(id) = event.get("session_id").and_then(Value::as_str) {
                    session_id = Some(id.to_string());
                }
                for a in claude_activity(&event) {
                    on(a);
                }
                if event.get("type").and_then(Value::as_str) == Some("result") {
                    saw_result = true;
                    cost = event.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
                    final_text = event.get("result").and_then(Value::as_str).unwrap_or("").to_string();
                    result = event.get("structured_output").cloned().filter(Value::is_object);
                    if event.get("is_error").and_then(Value::as_bool) == Some(true) {
                        result = None;
                        result_error = true;
                    }
                }
            }
        }
        let status = child.wait();
        *done.lock().unwrap_or_else(|e| e.into_inner()) = true;
        drop(tracked);
        let err = err_reader.join().unwrap_or_default();
        if let Some(last) = &last_message {
            if let Ok(text) = std::fs::read_to_string(last) {
                final_text = text;
            }
        }
        let _ = std::fs::remove_dir_all(&scratch);

        if stopped() {
            return AgentResult { outcome: Outcome::Stopped, json: None, session_id, cost_usd: cost };
        }
        let json = result.or_else(|| json_in(&final_text));
        // A limit is what the CLI says, not what the agent wrote: with a result
        // event, only its text and stderr count — a brain about an API's rate
        // limits must not read as the account's. Without one (it crashed),
        // everything it printed.
        let said = match (saw_result, result_error) {
            // The CLI's own error: its words count.
            (true, true) => format!("{final_text}\n{err}"),
            // The agent finished, just not in the shape asked: its text is its
            // own, whatever it says about rate limits.
            (true, false) => err.clone(),
            (false, _) => format!("{final_text}\n{err}\n{all}"),
        };
        let outcome = match (&json, limits::classify(&said)) {
            (Some(_), _) => Outcome::Ok,
            (None, Some(Signal::Limit)) => Outcome::Limit { resume_at: limits::resume_at(self.root.as_deref(), &call.account_id) },
            (None, Some(Signal::NoCredits)) => Outcome::NoCredits,
            (None, None) => {
                let why = err.lines().chain(final_text.lines()).rev().find(|l| !l.trim().is_empty()).unwrap_or("el agente terminó sin respuesta").to_string();
                let code = status.ok().and_then(|s| s.code()).map(|c| format!(" (salida {c})")).unwrap_or_default();
                Outcome::Failed(format!("{why}{code}"))
            }
        };
        AgentResult { outcome, json, session_id, cost_usd: cost }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(write: bool, resume: Option<&str>) -> AgentCall {
        AgentCall {
            project: "p-agent-tests".into(),
            key: "k".into(),
            account_id: "a".into(),
            model: "sonnet".into(),
            cwd: PathBuf::from("/tmp/wt"),
            prompt: "haz esto".into(),
            schema: serde_json::json!({"type":"object"}),
            resume: resume.map(Into::into),
            mcp_config: None,
            write,
            timeout: Duration::from_secs(5),
        }
    }

    #[test]
    fn claude_resumes_the_same_conversation_and_reviewers_only_read() {
        let a = claude_args(&call(true, Some("sess-1")));
        assert!(a.ends_with(&["--resume".to_string(), "sess-1".to_string()]));
        assert!(a.contains(&"acceptEdits".to_string()));
        let r = claude_args(&call(false, None));
        assert!(r.contains(&"--disallowedTools".to_string()) && !r.contains(&"acceptEdits".to_string()));
    }

    #[test]
    fn codex_runs_exec_in_the_worktree_and_resumes_by_thread() {
        let a = codex_args(&call(false, Some("th-9")), std::path::Path::new("/s.json"), std::path::Path::new("/l.txt"));
        assert_eq!(&a[..3], &["exec", "resume", "th-9"]);
        assert!(a.windows(2).any(|w| w == ["-c", "sandbox_mode=\"read-only\""]));
        assert!(!a.contains(&"--cd".to_string()), "exec resume has no --cd");
        assert_eq!(a.last().unwrap(), "haz esto");
        let fresh = codex_args(&call(true, None), std::path::Path::new("/s.json"), std::path::Path::new("/l.txt"));
        assert!(fresh.windows(2).any(|w| w == ["--cd", "/tmp/wt"]));
        assert!(fresh.windows(2).any(|w| w == ["-c", "sandbox_mode=\"workspace-write\""]));
    }

    #[test]
    fn json_is_found_inside_prose_and_activity_inside_stream_json() {
        assert_eq!(json_in("Listo:\n```json\n{\"ok\":true}\n```").unwrap()["ok"], true);
        assert!(json_in("sin json").is_none());
        let ev = serde_json::json!({"type":"assistant","message":{"content":[
            {"type":"tool_use","name":"Read","input":{"file_path":"app/routers/citas.py"}},
            {"type":"tool_use","name":"Bash","input":{"command":"pytest -q"}},
            {"type":"text","text":"hola"}]}});
        assert_eq!(claude_activity(&ev), vec![Activity { kind: "read", text: "app/routers/citas.py".into() }, Activity { kind: "cmd", text: "pytest -q".into() }]);
    }

    /// The fake CLI: a script standing in for `claude`, printing stream-json.
    fn fake(script: &str) -> CliRunner {
        let dir = std::env::temp_dir().join(format!("hangar-fake-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("claude");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let mut launches = HashMap::new();
        launches.insert("a".to_string(), Launch { provider: "claude".into(), binary: bin.to_string_lossy().to_string(), env: vec![] });
        CliRunner { launches, root: None }
    }

    #[cfg(unix)]
    #[test]
    fn a_fake_cli_answers_limits_and_fails_like_the_real_one() {
        let mut c = call(true, None);
        c.cwd = std::env::temp_dir();
        let ok = fake(r#"echo '{"type":"system","session_id":"s1"}'; echo '{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Edit","input":{"file_path":"a.py"}}]}}'; echo '{"type":"result","is_error":false,"total_cost_usd":0.5,"session_id":"s1","structured_output":{"done":true}}'"#);
        let mut seen = Vec::new();
        let r = ok.run(&c, &mut |a| seen.push(a));
        assert_eq!((r.outcome, r.session_id.as_deref(), r.cost_usd), (Outcome::Ok, Some("s1"), 0.5));
        assert_eq!(r.json.unwrap()["done"], true);
        assert_eq!(seen[0].kind, "edit");

        let limit = fake(r#"echo '{"type":"system","session_id":"s2"}'; echo '{"type":"result","is_error":true,"result":"Claude AI usage limit reached|1761832800"}'; exit 1"#);
        let r = limit.run(&c, &mut |_| {});
        assert!(matches!(r.outcome, Outcome::Limit { .. }));
        assert_eq!(r.session_id.as_deref(), Some("s2"), "kept, to resume the same conversation");

        let broke = fake(r#"echo 'Credit balance is too low' >&2; exit 1"#);
        assert_eq!(broke.run(&c, &mut |_| {}).outcome, Outcome::NoCredits);

        // An agent whose own text talks about an API's rate limit is not an
        // account at its limit.
        let chatty = fake(r#"echo '{"type":"assistant","message":{"content":[{"type":"text","text":"PokeAPI tiene rate limit"}]}}'; echo '{"type":"result","is_error":false,"result":"Respeto el rate limit de PokeAPI","session_id":"s3"}'"#);
        assert!(matches!(chatty.run(&c, &mut |_| {}).outcome, Outcome::Failed(_)));

        let failed = fake(r#"echo 'boom: not logged in' >&2; exit 2"#);
        assert!(matches!(failed.run(&c, &mut |_| {}).outcome, Outcome::Failed(why) if why.contains("not logged in")));
    }
}
