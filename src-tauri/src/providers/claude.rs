use super::Mode;

pub const BINARY: &str = "claude";

/// Absolute path to the `claude` binary (see `providers::resolve_binary`).
pub fn binary() -> String {
    super::resolve_binary(BINARY)
}

/// Args to launch the interactive subscription login (`claude auth login`).
/// Prints a sign-in URL, opens the browser, then prompts for the OAuth code
/// on stdin. Writes credentials into `CLAUDE_CONFIG_DIR` on success.
pub fn login_args() -> Vec<String> {
    vec!["auth".to_string(), "login".to_string(), "--claudeai".to_string()]
}

/// Args to query auth state as JSON (`claude auth status --json`) — respects
/// `CLAUDE_CONFIG_DIR`, so it reports the state of one isolated account.
pub fn auth_status_args() -> Vec<String> {
    vec!["auth".to_string(), "status".to_string(), "--json".to_string()]
}

/// Args to sign out of an account (`claude auth logout`), scoped to a config dir.
pub fn logout_args() -> Vec<String> {
    vec!["auth".to_string(), "logout".to_string()]
}

/// Maps Hangar's session mode to Claude Code CLI's `--permission-mode` values.
/// TODO: re-verify these exact flag names/values against the installed CLI
/// version — this is the "flags investigated during phase 2" item from the plan.
fn permission_mode_str(mode: Mode) -> &'static str {
    match mode {
        Mode::Plan => "plan",
        Mode::AskEdits => "default",
        Mode::AutoEdit => "acceptEdits",
        Mode::Yolo => "bypassPermissions",
    }
}

/// For interactive-PTY use (see process_manager::spawn) — not used by the
/// headless turn runner, which builds its own args via `build_turn_args`.
#[allow(dead_code)]
pub fn build_args(mode: Mode, prompt: Option<&str>) -> Vec<String> {
    let mut args = vec!["--permission-mode".to_string(), permission_mode_str(mode).to_string()];
    if let Some(p) = prompt {
        args.push(p.to_string());
    }
    args
}

pub struct TurnArgs<'a> {
    pub mode: Mode,
    pub prompt: &'a str,
    pub resume: Option<&'a str>,
    /// `--model` value — a CLI alias ("opus", "sonnet") or full model id
    /// ("claude-opus-4-8"), as returned by `list_provider_models`.
    pub model: &'a str,
    /// `--effort` value: one of low/medium/high/xhigh/max.
    pub effort: &'a str,
    /// `--name` — shows up in the CLI's own `/resume` picker and terminal
    /// title, separate from (but usually equal to) Hangar's own session name.
    pub name: &'a str,
    /// Set only for `Mode::AskEdits`/`Mode::AutoEdit` (the only modes that
    /// can still block on live approval in headless `-p` mode). When true,
    /// `build_turn_args` wires up Claude Code's own stdio control protocol
    /// (`--permission-prompt-tool stdio` + `--input-format stream-json`) —
    /// the same mechanism its Agent SDK's `canUseTool` and the VS Code
    /// extension use — instead of passing the prompt as a `-p` argument.
    /// Verified empirically against a real `claude` 2.1.212 install: a
    /// `control_request` JSON line appears on stdout for a blocked tool
    /// call, and Claude Code blocks on its own stdin (no timeout of its
    /// own) until a matching `control_response` line arrives there — see
    /// the plan this shipped with for the exact transcript. `false` for
    /// every other mode, which never blocks on approval in the first place.
    pub use_permission_prompt: bool,
    /// Path to the user's standing preferences (conversation and code
    /// language), or `None` when nothing is configured — in which case no flag
    /// is added at all rather than one pointing at an empty file.
    pub instructions: Option<&'a std::path::Path>,
    /// Path to the `--mcp-config` naming this scope's code graph, or `None`
    /// when there is no graph built or graphify isn't installed
    /// (`GRAPHIFY_TRD.md` §5.2). Additive: Hangar never passes
    /// `--strict-mcp-config`, which would suppress the user's own servers.
    pub mcp_config: Option<&'a std::path::Path>,
}

/// Args for one headless turn: `claude -p "<prompt>" --output-format
/// stream-json`, optionally continuing a prior conversation via `--resume`.
/// When `use_permission_prompt` is set, the prompt moves off the command
/// line and onto stdin instead (see `turn_runner::spawn_turn`), since
/// `--input-format stream-json` expects the initial message there.
pub fn build_turn_args(args: TurnArgs) -> Vec<String> {
    let mut out = vec!["-p".to_string()];
    if !args.use_permission_prompt {
        out.push(args.prompt.to_string());
    }
    out.push("--output-format".to_string());
    out.push("stream-json".to_string());
    out.push("--verbose".to_string());
    out.push("--permission-mode".to_string());
    out.push(permission_mode_str(args.mode).to_string());
    if args.use_permission_prompt {
        out.push("--permission-prompt-tool".to_string());
        out.push("stdio".to_string());
        out.push("--input-format".to_string());
        out.push("stream-json".to_string());
    }
    if !args.model.trim().is_empty() {
        out.push("--model".to_string());
        out.push(args.model.to_string());
    }
    if !args.effort.trim().is_empty() {
        out.push("--effort".to_string());
        out.push(args.effort.to_string());
    }
    if !args.name.trim().is_empty() {
        out.push("--name".to_string());
        out.push(args.name.to_string());
    }
    if let Some(r) = args.resume {
        out.push("--resume".to_string());
        out.push(r.to_string());
    }
    // Always "claude" here — this builder is Claude Code's own — but routed
    // through `instruction_args` anyway so the flag is named in exactly one
    // place for every caller and provider.
    if let Some(path) = args.instructions {
        // The inline form is empty because this builder is Claude Code's own,
        // and Claude Code takes the file. Only Codex needs the flattened text,
        // and Codex has no headless turn runner.
        out.extend(super::instruction_args("claude", &path.to_string_lossy(), ""));
    }
    // Same routing, same reason: the flag is named in one place. `None` means
    // no flag at all — never one pointing at a graph that isn't there.
    if let Some(config) = args.mcp_config {
        out.extend(super::mcp_config_args("claude", &config.to_string_lossy()));
    }
    out
}

/// Isolates a Claude Code login to this account's own config dir, so two
/// accounts on this provider don't share/clobber the same login session.
/// `api_key` is set for "key"-billed accounts (fetched from the OS keychain
/// by the caller) and left `None` for subscription accounts, which rely on
/// the CLI login stored under `config_dir`.
pub fn env_for_account(config_dir: &str, api_key: Option<&str>) -> Vec<(String, String)> {
    let mut env = vec![("CLAUDE_CONFIG_DIR".to_string(), config_dir.to_string())];
    if let Some(key) = api_key {
        env.push(("ANTHROPIC_API_KEY".to_string(), key.to_string()));
    }
    env
}

/// Model aliases Claude Code CLI accepts for `--model`. Used directly for
/// subscription accounts (no raw API key to query the live catalog with)
/// and as a fallback if the live `/v1/models` fetch fails for any reason.
pub fn known_model_aliases() -> Vec<crate::models::ModelOption> {
    vec![
        crate::models::ModelOption { id: "opus".to_string(), label: "Opus 4.8".to_string() },
        crate::models::ModelOption { id: "sonnet".to_string(), label: "Sonnet 5".to_string() },
        crate::models::ModelOption { id: "haiku".to_string(), label: "Haiku 4.5".to_string() },
        crate::models::ModelOption { id: "fable".to_string(), label: "Fable 5".to_string() },
    ]
}

const EXHAUSTION_MARKERS: &[&str] = &[
    "usage limit reached",
    "rate limit",
    "rate_limit_error",
    "please try again later",
];

pub fn is_exhaustion_signal(line: &str) -> bool {
    let lower = line.to_lowercase();
    EXHAUSTION_MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_turn_args_includes_model_effort_and_name_flags() {
        let args = build_turn_args(TurnArgs {
            mode: Mode::AutoEdit,
            prompt: "hello",
            resume: None,
            model: "opus",
            effort: "high",
            name: "fix-billing-rounding",
            use_permission_prompt: false,
            instructions: None,
            mcp_config: None,
        });
        assert_eq!(
            args,
            vec![
                "-p", "hello", "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits",
                "--model", "opus", "--effort", "high", "--name", "fix-billing-rounding",
            ]
        );
    }

    #[test]
    fn build_turn_args_includes_resume_when_continuing() {
        let args = build_turn_args(TurnArgs {
            mode: Mode::Plan,
            prompt: "continue",
            resume: Some("session-123"),
            model: "sonnet",
            effort: "medium",
            name: "",
            use_permission_prompt: false,
            instructions: None,
            mcp_config: None,
        });
        assert_eq!(args.last().cloned(), Some("session-123".to_string()));
        assert_eq!(args[args.len() - 2], "--resume");
        // Empty name must be omitted rather than passing `--name ""`.
        assert!(!args.contains(&"--name".to_string()));
    }

    #[test]
    fn build_turn_args_omits_stdio_flags_when_permission_prompt_not_needed() {
        let args = build_turn_args(TurnArgs {
            mode: Mode::AskEdits,
            prompt: "hi",
            resume: None,
            model: "",
            effort: "",
            name: "",
            use_permission_prompt: false,
            instructions: None,
            mcp_config: None,
        });
        assert!(args.contains(&"hi".to_string()), "prompt must be a positional arg when not using the stdio protocol");
        assert!(!args.contains(&"--permission-prompt-tool".to_string()));
        assert!(!args.contains(&"--input-format".to_string()));
    }

    #[test]
    fn build_turn_args_uses_stdio_control_protocol_and_moves_prompt_off_the_command_line() {
        let args = build_turn_args(TurnArgs {
            mode: Mode::AskEdits,
            prompt: "hi",
            resume: None,
            model: "",
            effort: "",
            name: "",
            use_permission_prompt: true,
            instructions: None,
            mcp_config: None,
        });
        // The prompt must NOT appear as a `-p` positional arg — it goes over
        // stdin instead (see turn_runner::spawn_turn).
        assert!(!args.contains(&"hi".to_string()));
        assert!(args.contains(&"-p".to_string()));
        let tool_index = args.iter().position(|a| a == "--permission-prompt-tool").expect("must include --permission-prompt-tool");
        assert_eq!(args[tool_index + 1], "stdio");
        let format_index = args.iter().position(|a| a == "--input-format").expect("must include --input-format");
        assert_eq!(args[format_index + 1], "stream-json");
    }

    #[test]
    fn known_model_aliases_are_valid_cli_aliases() {
        let aliases = known_model_aliases();
        let ids: Vec<_> = aliases.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"opus"));
        assert!(ids.contains(&"sonnet"));
    }
}
