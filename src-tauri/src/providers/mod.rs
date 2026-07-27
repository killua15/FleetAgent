pub mod claude;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Read-only — proposes changes but never edits or executes. Hangar's
    /// addition beyond the mockup's ask-edits/auto-edit/yolo enum, covering
    /// the "plan mode, solo análisis" requirement from the original ask.
    Plan,
    AskEdits,
    AutoEdit,
    Yolo,
}

impl Mode {
    pub fn from_str(s: &str) -> Self {
        match s {
            "plan" => Mode::Plan,
            "auto_edit" => Mode::AutoEdit,
            "yolo" => Mode::Yolo,
            _ => Mode::AskEdits,
        }
    }
}

/// Best-effort exhaustion/rate-limit detection, dispatched per provider so
/// process_manager stays provider-agnostic. Exact strings should be
/// re-verified against each CLI's real output as adapters are added.
pub fn is_exhaustion_signal(provider: &str, line: &str) -> bool {
    match provider {
        "claude" => claude::is_exhaustion_signal(line),
        _ => false,
    }
}

/// Resolves a CLI binary to an absolute path. GUI apps launched from Finder
/// inherit a minimal PATH that usually excludes `~/.local/bin`, homebrew,
/// etc., so relying on the bare name in PATH breaks the packaged app. We
/// search PATH first, then the common install locations, and fall back to
/// the bare name so a PATH-based spawn can still work in dev.
pub fn resolve_binary(name: &str) -> String {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return candidate.to_string_lossy().into_owned();
            }
        }
    }

    let home = std::env::var("HOME").unwrap_or_default();
    let candidates = [
        format!("{home}/.local/bin/{name}"),
        format!("/opt/homebrew/bin/{name}"),
        format!("/usr/local/bin/{name}"),
        format!("{home}/.claude/local/{name}"),
    ];
    for candidate in candidates {
        if std::path::Path::new(&candidate).is_file() {
            return candidate;
        }
    }

    name.to_string()
}

/// Everything Hangar needs to run one provider's CLI as a specific account.
///
/// Exists because the alternative was `if provider != "claude"` in three
/// unrelated files plus two one-armed `match`es — which meant adding a second
/// provider touched every one of them, and the failure mode was a *misleading*
/// one: the login refused with "its CLI isn't installed on this machine" when
/// the CLI was installed and the real reason was that nobody had wired it.
#[derive(Debug, Clone, Copy)]
pub struct ProviderSpec {
    /// The binary that actually runs. Note that this is not always the
    /// provider's own name: Kimi is served through `claude`, because Moonshot
    /// exposes an Anthropic-compatible API and Claude Code speaks it.
    pub binary: &'static str,
    /// The environment variable that isolates one account's config and
    /// credentials from another's. This is the whole basis of Hangar's
    /// multi-account model, and it's per-CLI: `CLAUDE_CONFIG_DIR` for Claude
    /// Code, `CODEX_HOME` for Codex.
    pub config_dir_env: &'static str,
    /// Args for the interactive subscription login, or `None` when the
    /// provider has no such thing and authenticates with a key instead.
    pub login_args: Option<&'static [&'static str]>,
    /// Fixed environment this provider always needs — how a Kimi account
    /// points Claude Code at Moonshot instead of Anthropic.
    pub extra_env: &'static [(&'static str, &'static str)],
    /// Name of the variable an API key is passed in, for key-billed accounts.
    pub key_env: &'static str,
    /// How to ask this CLI whether the config dir is logged in.
    pub auth_status: AuthStatusProbe,
    /// How this CLI lets Hangar address a conversation.
    pub session: SessionControl,
    /// Whether the interactive login ends by asking for a code pasted back.
    ///
    /// Claude prints a URL and waits on stdin; Codex runs a loopback OAuth flow
    /// that completes by itself when the browser redirects. Showing a paste box
    /// for the second is asking for something that will never exist — which is
    /// exactly what made the Codex login look stuck.
    pub login_expects_code: bool,
}

/// Every CLI answers "am I logged in?" differently, and there is no
/// standard — Claude has a `--json` flag, Codex prints a sentence.
#[derive(Debug, Clone, Copy)]
pub enum AuthStatusProbe {
    /// Run these args and read `loggedIn`/`authMethod`/`email` out of JSON.
    Json(&'static [&'static str]),
    /// Run these args and treat stdout containing `logged_marker` as success.
    /// Coarser, and honest about it: it yields no email or plan, so the UI
    /// shows the account name the user typed instead of inventing detail.
    Text { args: &'static [&'static str], logged_marker: &'static str },
    /// No way to ask — a key-billed provider is "logged in" when it has a key.
    None,
}

/// How a CLI lets a conversation be named and resumed — the difference that
/// decides whether Hangar can transfer one exactly or only approximately.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SessionControl {
    /// Hangar chooses the id up front and resumes it by name. This is what
    /// makes exact transcript transfer between accounts possible: knowing the
    /// id means knowing which file to copy.
    MintableId { start_flag: &'static str, resume_flag: &'static str },
    /// The CLI assigns its own id. Verified against `codex --help`: there is no
    /// flag to name a session at start, and `codex resume [SESSION_ID]` is the
    /// only way back — so Hangar can't mint one, and resuming means finding it
    /// afterwards rather than choosing it.
    AssignedId { resume_subcommand: &'static str },
    /// No session concept exposed.
    None,
}

/// A file whose presence in the config dir means "logged in", checked before
/// spawning anything.
///
/// Belt and braces for the probe, and cheaper: `codex login status` is a
/// process spawn parsed by matching a sentence, while `auth.json` appearing is
/// what actually happened. Every community tool that switches Codex accounts
/// works by moving that file, which is a good sign it's the durable signal and
/// the sentence is the fragile one.
pub fn auth_marker_file(provider: &str) -> Option<&'static str> {
    match provider {
        "codex" => Some("auth.json"),
        _ => None,
    }
}

const CLAUDE_LOGIN: &[&str] = &["auth", "login", "--claudeai"];
const CLAUDE_STATUS: &[&str] = &["auth", "status", "--json"];
const CODEX_LOGIN: &[&str] = &["login"];
/// Verified against the installed CLI: prints "Not logged in" or "Logged in
/// using ChatGPT", and respects `CODEX_HOME`.
const CODEX_STATUS: &[&str] = &["login", "status"];
const GEMINI_LOGIN: &[&str] = &["auth", "login"];

/// Moonshot's Anthropic-compatible endpoint. A Kimi account *is* Claude Code
/// pointed somewhere else, which is why it inherits every feature built for
/// Claude — session ids, resume, transcript transfer — without an adapter.
const KIMI_ENV: &[(&str, &str)] = &[("ANTHROPIC_BASE_URL", "https://api.moonshot.ai/anthropic")];

pub fn spec(provider: &str) -> Option<ProviderSpec> {
    match provider {
        "claude" => Some(ProviderSpec {
            binary: claude::BINARY,
            config_dir_env: "CLAUDE_CONFIG_DIR",
            login_args: Some(CLAUDE_LOGIN),
            extra_env: &[],
            key_env: "ANTHROPIC_API_KEY",
            auth_status: AuthStatusProbe::Json(CLAUDE_STATUS),
            session: SessionControl::MintableId { start_flag: "--session-id", resume_flag: "--resume" },
            login_expects_code: true,
        }),
        "codex" => Some(ProviderSpec {
            binary: "codex",
            config_dir_env: "CODEX_HOME",
            login_args: Some(CODEX_LOGIN),
            extra_env: &[],
            key_env: "OPENAI_API_KEY",
            auth_status: AuthStatusProbe::Text { args: CODEX_STATUS, logged_marker: "Logged in" },
            session: SessionControl::AssignedId { resume_subcommand: "resume" },
            // Loopback OAuth: the browser redirect finishes it, there is
            // nothing to paste.
            login_expects_code: false,
        }),
        "gemini" => Some(ProviderSpec {
            binary: "gemini",
            config_dir_env: "GEMINI_CONFIG_DIR",
            login_args: Some(GEMINI_LOGIN),
            extra_env: &[],
            key_env: "GEMINI_API_KEY",
            auth_status: AuthStatusProbe::None,
            session: SessionControl::None,
            login_expects_code: false,
        }),
        // Kimi has no interactive subscription login here: it's a Moonshot API
        // key, so the wizard's key path is the only one that applies.
        "kimi" => Some(ProviderSpec {
            binary: claude::BINARY,
            config_dir_env: "CLAUDE_CONFIG_DIR",
            login_args: None,
            extra_env: KIMI_ENV,
            key_env: "ANTHROPIC_AUTH_TOKEN",
            auth_status: AuthStatusProbe::None,
            // It *is* Claude Code, so it addresses conversations the same way.
            session: SessionControl::MintableId { start_flag: "--session-id", resume_flag: "--resume" },
            login_expects_code: false,
        }),
        _ => None,
    }
}

/// The CLI a provider is driven through, or `None` for a provider Hangar
/// doesn't know. Single source of truth so "is it installed?" and "what do I
/// run?" can never disagree about which binary a provider means.
pub fn provider_binary(provider: &str) -> Option<&'static str> {
    spec(provider).map(|s| s.binary)
}

/// Whether a provider's CLI is installed and resolvable on this machine.
/// Used by the Add-account wizard to gate the subscription-login step and
/// flag providers whose CLI is missing. `resolve_binary` returns an
/// absolute path when found and the bare name otherwise, so a resolved
/// value different from the bare name means it was located.
pub fn is_provider_available(provider: &str) -> bool {
    let Some(binary) = provider_binary(provider) else { return false };
    resolve_binary(binary) != binary
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The isolation variable is the whole basis of the multi-account model,
    /// and it's per-CLI. Getting it wrong doesn't fail — it silently writes a
    /// Codex login into the shared `~/.codex`, so every "isolated" account ends
    /// up sharing credentials.
    #[test]
    fn each_provider_isolates_with_its_own_variable() {
        assert_eq!(spec("claude").unwrap().config_dir_env, "CLAUDE_CONFIG_DIR");
        assert_eq!(spec("codex").unwrap().config_dir_env, "CODEX_HOME");
        assert_eq!(spec("gemini").unwrap().config_dir_env, "GEMINI_CONFIG_DIR");
    }

    /// Kimi is Claude Code pointed elsewhere. If the binary ever stopped being
    /// `claude`, it would quietly lose session ids, resume and transcript
    /// transfer — every feature it inherits for free.
    #[test]
    fn kimi_runs_through_claude_against_moonshot() {
        let kimi = spec("kimi").unwrap();
        assert_eq!(kimi.binary, "claude");
        assert_eq!(kimi.key_env, "ANTHROPIC_AUTH_TOKEN");
        assert!(kimi.extra_env.iter().any(|(k, v)| *k == "ANTHROPIC_BASE_URL" && v.contains("moonshot")));
        assert!(kimi.login_args.is_none(), "there is no interactive Kimi login to offer");
    }

    /// The two halves of the Codex bug, as assertions: it has a login, and that
    /// login doesn't end with a code to paste.
    #[test]
    fn codex_has_a_login_that_completes_without_a_pasted_code() {
        let codex = spec("codex").unwrap();
        assert!(codex.login_args.is_some());
        assert!(!codex.login_expects_code, "codex finishes through the browser redirect");
        assert!(spec("claude").unwrap().login_expects_code, "claude does wait on stdin for one");
    }

    #[test]
    fn an_unknown_provider_has_no_spec_rather_than_a_default_one() {
        assert!(spec("copilot").is_none(), "guessing a binary for a provider we don't know would run something arbitrary");
    }

    /// `resolve_binary` is tested against a binary this test plants itself on
    /// a temporary PATH, rather than against whatever CLIs happen to be
    /// installed. Asserting "claude resolves to an absolute path" only passed
    /// on a machine with Claude Code installed — it failed on any CI runner
    /// and on any contributor's machine without it, testing the environment
    /// instead of the function.
    ///
    /// PATH is process-global, so this test also can't run concurrently with
    /// anything else that reads it — hence one test covering both the found
    /// and not-found cases instead of two racing ones.
    #[test]
    fn resolves_a_binary_on_path_to_its_absolute_location() {
        let dir = std::env::temp_dir().join(format!("hangar-test-path-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake_cli = dir.join("hangar-fake-cli");
        std::fs::write(&fake_cli, "#!/bin/sh\nexit 0\n").unwrap();

        let original_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{original_path}", dir.display()));

        let resolved = resolve_binary("hangar-fake-cli");
        let unresolvable = resolve_binary("definitely-not-a-real-cli-xyz");

        std::env::set_var("PATH", original_path);

        assert_eq!(resolved, fake_cli.to_string_lossy(), "a binary on PATH must resolve to its absolute location");
        assert!(std::path::Path::new(&resolved).is_absolute());
        assert_eq!(unresolvable, "definitely-not-a-real-cli-xyz", "an unresolvable name must fall back to the bare name");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Only the provider *mapping* is machine-independent — whether a given
    /// CLI is installed is not something to assert on.
    #[test]
    fn unknown_providers_are_never_reported_as_available() {
        assert!(!is_provider_available("not-a-provider"));
        assert!(!is_provider_available(""));
    }
}
