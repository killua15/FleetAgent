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

/// Los argumentos con los que cada CLI aplica un modo de permisos.
///
/// **Verificado contra los CLIs instalados**, porque los nombres no se parecen
/// entre sí y ninguno se puede adivinar: Claude usa `--permission-mode` con
/// valores en camelCase (`bypassPermissions`), y Codex separa la decisión en dos
/// ejes — cuándo pregunta (`--ask-for-approval`) y qué puede escribir
/// (`--sandbox`). Pasarle a uno la bandera del otro es `error: unexpected
/// argument`, que es exactamente cómo empezó el soporte de Codex.
///
/// `yolo` es el modo que sostiene el diseño móvil (`MOBILE_BRIDGE_TRD.md` §13):
/// sin permisos que aprobar no hace falta que nada te despierte. Y es el modo en
/// el que un agente puede destruir algo, así que lo que protege pasa a ser el
/// worktree aislado y git.
pub fn permission_args(provider: &str, mode: Mode) -> &'static [&'static str] {
    // ASSUMPTION: grok's CLI hasn't been verified against these flags yet
    // (unlike claude/codex/kimi, checked against the installed binary per the
    // comment above) — bucketed with claude for now since it's the more
    // common modern convention. Re-verify against `grok --help` and move it
    // to its own branch below if it doesn't match.
    let claude_like = matches!(provider, "claude" | "kimi" | "grok");
    match (claude_like, mode) {
        (true, Mode::Plan) => &["--permission-mode", "plan"],
        (true, Mode::AskEdits) => &["--permission-mode", "manual"],
        (true, Mode::AutoEdit) => &["--permission-mode", "acceptEdits"],
        (true, Mode::Yolo) => &["--permission-mode", "bypassPermissions"],

        // Codex necesita los dos ejes: sin `--sandbox` seguiría sin poder
        // escribir aunque no pregunte, y "no pregunta pero no puede hacer nada"
        // es la peor combinación posible.
        (false, Mode::Plan) => &["--ask-for-approval", "untrusted", "--sandbox", "read-only"],
        (false, Mode::AskEdits) => &["--ask-for-approval", "untrusted"],
        (false, Mode::AutoEdit) => &["--ask-for-approval", "on-request", "--sandbox", "workspace-write"],
        (false, Mode::Yolo) => &["--ask-for-approval", "never", "--sandbox", "danger-full-access"],
    }
}

/// How each CLI takes a block of standing instructions — the channel Hangar's
/// per-project preferences travel through.
///
/// **Verified against the installed CLIs**, and they do not agree. Claude Code
/// has `--append-system-prompt-file`, confirmed by running it: the same English
/// question answered in Spanish with the flag and in English without it. Codex
/// has no equivalent flag at all — its documented mechanism is `AGENTS.md` in
/// the repo tree, which would mean writing into the user's checkout, so it is
/// left out rather than faked.
///
/// The **file** variant, not the inline one, because
/// `terminals::account_command_line_with_mode` renders a *shell string*: a
/// multi-line, accented instruction block inlined there is exactly what trips
/// zsh's PS2 continuation. A path has none of those characters.
///
/// Returns empty for a provider with no channel, which the callers treat as
/// "add nothing" — a terminal for such a provider opens exactly as it does
/// today.
/// `inline` is the same content flattened to a single line, for a CLI that
/// takes the text rather than a path — see below for why that matters.
pub fn instruction_args(provider: &str, path: &str, inline: &str) -> Vec<String> {
    match provider {
        // Kimi and Grok run through Claude Code's own binary/conventions, the
        // same bucketing `permission_args` uses above.
        "claude" | "kimi" | "grok" => vec!["--append-system-prompt-file".to_string(), path.to_string()],
        // Codex has no flag for this, but it does have a config key, and
        // `-c` overrides anything `~/.codex/config.toml` would have said.
        //
        // `developer_instructions` and **not** `model_instructions_file`: the
        // latter is documented as a *replacement* for the CLI's built-in
        // instructions, so using it would trade the user's language preference
        // for Codex's own idea of how to behave.
        //
        // The value has to be one line — this ends up inside a shell string
        // (see `terminals::account_command_line_full`), where a newline opens
        // zsh's continuation prompt.
        "codex" if !inline.trim().is_empty() => vec!["-c".to_string(), format!("developer_instructions={inline}")],
        _ => Vec::new(),
    }
}

/// How each CLI is pointed at a directory of skills.
///
/// `dir` is a directory *containing* `.claude/skills/`, not the skills folder
/// itself — that is the shape `--add-dir` expects.
///
/// **Verified by running it**: a canary skill carrying a unique token came
/// back from the agent with `--add-dir` and was unknown without it. Claude
/// Code's docs call this out as a deliberate exception — `--add-dir` otherwise
/// grants file access without loading configuration, but `.claude/skills/`
/// inside an added directory is loaded anyway.
///
/// Codex reads `.agents/skills`, not `.claude/skills`, and takes explicit
/// per-skill paths instead of a parent directory. Left empty until that can be
/// verified against a real run: this account's Codex quota is exhausted, and
/// shipping an unverified `-c` risks the `error: unexpected argument` failure
/// that Codex support started life with. A Codex terminal behaves exactly as
/// it does today.
pub fn skills_args(provider: &str, dir: &str) -> Vec<String> {
    match provider {
        "claude" | "kimi" | "grok" => vec!["--add-dir".to_string(), dir.to_string()],
        _ => Vec::new(),
    }
}

/// How each CLI is handed the code knowledge graph's MCP server
/// (`GRAPHIFY_TRD.md` §5.2).
///
/// **Additive, never `--strict-mcp-config`.** That flag makes Claude Code
/// ignore every other MCP configuration, which would mean switching off the
/// user's own servers for the privilege of adding ours. Unacceptable: additive
/// or nothing.
///
/// A flag rather than a registered server, for two reasons. Registering at user
/// level would mean writing into every account's `CLAUDE_CONFIG_DIR` — the
/// directories that exist precisely so accounts don't contaminate each other —
/// and rewriting them on every new account. And a `.mcp.json` in the project
/// would be a file in the user's checkout, which §3 forbids.
///
/// Empty for a provider with no equivalent, which callers treat as "add
/// nothing": that terminal opens exactly as it does today.
pub fn mcp_config_args(provider: &str, config_path: &str) -> Vec<String> {
    match provider {
        "claude" | "kimi" | "grok" => vec!["--mcp-config".to_string(), config_path.to_string()],
        _ => Vec::new(),
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
    if let Ok(cached) = RESOLVED.lock() {
        if let Some(path) = cached.get(name) {
            return path.clone();
        }
    }
    let resolved = resolve_binary_uncached(name);
    if let Ok(mut cached) = RESOLVED.lock() {
        cached.insert(name.to_string(), resolved.clone());
    }
    resolved
}

/// Resolving can end up spawning a login shell, which costs a few hundred
/// milliseconds — and `resolve_binary` is called on every terminal open, every
/// auth probe and every `gh` call.
static RESOLVED: std::sync::Mutex<std::collections::BTreeMap<String, String>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Drops the resolution cache so a tool installed while Hangar is running is
/// found without a relaunch.
///
/// The cache exists because resolving can spawn a login shell and this is
/// called on every terminal open. But it also means a user who follows an
/// install hint keeps being told the tool is missing — which reads as "I did
/// what it said and nothing happened". Cleared only on explicit re-check, not
/// on a timer: the steady state is that nothing changes.
pub fn forget_resolved_binaries() {
    if let Ok(mut cached) = RESOLVED.lock() {
        cached.clear();
    }
}

fn resolve_binary_uncached(name: &str) -> String {
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
        // Version managers that keep binaries outside any fixed directory.
        format!("{home}/.volta/bin/{name}"),
        format!("{home}/.bun/bin/{name}"),
        format!("{home}/.asdf/shims/{name}"),
        format!("{home}/.local/share/mise/shims/{name}"),
    ];
    for candidate in candidates {
        if std::path::Path::new(&candidate).is_file() {
            return candidate;
        }
    }

    // nvm keeps a directory per Node version and puts every globally installed
    // CLI inside it — which is where `codex` lands for anyone who installed
    // Node with nvm, and it's reachable from a terminal (the shell profile adds
    // it) but not from an app launched by Finder.
    if let Some(found) = newest_nvm_binary(&home, name) {
        return found;
    }

    // Last resort before giving up: ask the user's own login shell. It knows
    // about every version manager, including ones invented after this code was
    // written, because it's the thing that sets them up. `-ilc` rather than
    // `-lc` on purpose: nvm is initialised in `.zshrc`, which only an
    // interactive shell reads.
    if let Some(found) = ask_login_shell(name) {
        return found;
    }

    name.to_string()
}

/// The newest Node version that has this binary.
///
/// Newest rather than first because nvm directories sort by name and versions
/// don't: `v9.0.0` sorts after `v24.18.0`, so taking the last alphabetically
/// would pick an ancient Node with no `codex` in it.
fn newest_nvm_binary(home: &str, name: &str) -> Option<String> {
    let versions = std::fs::read_dir(format!("{home}/.nvm/versions/node")).ok()?;
    let mut best: Option<((u32, u32, u32), String)> = None;
    for entry in versions.flatten() {
        let candidate = entry.path().join("bin").join(name);
        if !candidate.is_file() {
            continue;
        }
        let version = parse_node_version(&entry.file_name().to_string_lossy());
        if best.as_ref().is_none_or(|(current, _)| version > *current) {
            best = Some((version, candidate.to_string_lossy().into_owned()));
        }
    }
    best.map(|(_, path)| path)
}

fn parse_node_version(dir: &str) -> (u32, u32, u32) {
    let mut parts = dir.trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0));
    (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0))
}

fn ask_login_shell(name: &str) -> Option<String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let mut cmd = std::process::Command::new(shell);
    cmd.args(["-ilc", &format!("command -v {name}")]);
    let output = crate::process_timeout::run_with_timeout(cmd, std::time::Duration::from_secs(10)).ok()?;
    // An interactive shell prints whatever the profile prints, so the answer is
    // the last line that looks like a path rather than the whole output.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('/') && std::path::Path::new(line).is_file())
        .map(str::to_string)
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
/// ASSUMPTION, not verified against a real `grok` CLI (none of the other
/// consts on this page are guesses — this one is). `grok auth login` is what
/// was described when this provider was added; re-check against `grok --help`
/// once the binary is actually available.
const GROK_LOGIN: &[&str] = &["auth", "login"];

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
        // ASSUMPTION: config_dir_env, key_env, auth probe and session control
        // are all unverified against a real `grok` CLI — modeled on Gemini's
        // shape (own binary, own login, but no way yet to ask "am I logged
        // in?" or to name/resume a session) since that's the closest existing
        // provider with its own dedicated binary and no special-cased
        // integration. Tighten these once the real CLI's behavior is known.
        "grok" => Some(ProviderSpec {
            binary: "grok",
            config_dir_env: "GROK_CONFIG_DIR",
            login_args: Some(GROK_LOGIN),
            extra_env: &[],
            key_env: "XAI_API_KEY",
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

    /// nvm directories sort by name and versions don't. Taking the last one
    /// alphabetically picks `v9` over `v24` — an ancient Node that has none of
    /// the CLIs installed, which reads to the user as "codex isn't installed"
    /// while `codex` runs fine in their terminal.
    #[test]
    fn the_newest_node_wins_even_though_it_sorts_first() {
        assert!(parse_node_version("v24.18.0") > parse_node_version("v9.11.2"));
        assert!(parse_node_version("v22.0.0") > parse_node_version("v20.19.4"));
        assert_eq!(parse_node_version("nonsense"), (0, 0, 0), "an unparseable directory must lose, not panic");
    }

    /// Finds a binary that only exists inside an nvm version directory —
    /// exactly where a globally installed `codex` lands, and exactly where an
    /// app launched from Finder can't see it.
    #[test]
    fn a_binary_only_installed_under_nvm_is_still_found() {
        let home = std::env::temp_dir().join(format!("hangar-test-nvm-{}", uuid::Uuid::new_v4()));
        let old = home.join(".nvm/versions/node/v9.11.2/bin");
        let new = home.join(".nvm/versions/node/v24.18.0/bin");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("codex"), "#!/bin/sh\n").unwrap();

        let found = newest_nvm_binary(&home.to_string_lossy(), "codex");
        assert_eq!(found.as_deref(), Some(new.join("codex").to_string_lossy().as_ref()));
        assert_eq!(newest_nvm_binary(&home.to_string_lossy(), "not-installed"), None);

        let _ = std::fs::remove_dir_all(&home);
    }

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

    /// Los nombres no se parecen entre sí y ninguno se puede adivinar. Pasarle a
    /// un CLI la bandera del otro es `error: unexpected argument`, que es
    /// exactamente cómo empezó el soporte de Codex.
    #[test]
    fn cada_cli_recibe_las_banderas_que_de_verdad_entiende() {
        assert_eq!(permission_args("claude", Mode::Yolo), &["--permission-mode", "bypassPermissions"]);
        // Kimi *es* Claude Code apuntando a Moonshot, así que hereda las suyas.
        assert_eq!(permission_args("kimi", Mode::Plan), &["--permission-mode", "plan"]);

        let codex = permission_args("codex", Mode::Yolo);
        assert!(codex.contains(&"--ask-for-approval"), "{codex:?}");
        assert!(!codex.contains(&"--permission-mode"), "la bandera de Claude no existe en Codex: {codex:?}");
    }

    /// Mismo cuidado que con los permisos: la bandera de instrucciones de
    /// Claude no existe en Codex, y pasársela abriría la terminal con
    /// `error: unexpected argument`. Vacío significa "no añadas nada", no
    /// "añade algo vacío".
    #[test]
    fn cada_cli_recibe_las_instrucciones_en_la_forma_que_acepta() {
        let inline = "Responde en español.";
        assert_eq!(instruction_args("claude", "/tmp/p.md", inline), vec!["--append-system-prompt-file", "/tmp/p.md"]);
        // Kimi y Grok van por el binario y las convenciones de Claude Code.
        assert_eq!(instruction_args("kimi", "/tmp/p.md", inline), vec!["--append-system-prompt-file", "/tmp/p.md"]);
        assert_eq!(instruction_args("grok", "/tmp/p.md", inline), vec!["--append-system-prompt-file", "/tmp/p.md"]);

        // Codex no tiene bandera, pero sí una clave de configuración, y toma el
        // texto en vez de una ruta. Pasarle la bandera de Claude sería
        // `error: unexpected argument`.
        let codex = instruction_args("codex", "/tmp/p.md", inline);
        assert_eq!(codex, vec!["-c", "developer_instructions=Responde en español."]);
        assert!(!codex.contains(&"--append-system-prompt-file".to_string()));
        // `model_instructions_file` reemplazaría las instrucciones internas del
        // CLI en vez de sumarse, así que no debe aparecer nunca.
        assert!(!codex.iter().any(|a| a.contains("model_instructions_file")));

        // Sin nada que decir, ninguna clave: `-c developer_instructions=` sería
        // gastar una bandera para no decir nada.
        assert!(instruction_args("codex", "/tmp/p.md", "").is_empty());
        assert!(instruction_args("gemini", "/tmp/p.md", inline).is_empty());
    }

    /// `--add-dir` es lo que carga `.claude/skills/` del directorio añadido —
    /// verificado con una skill "canario" que el agente devolvió con la bandera
    /// y desconocía sin ella.
    #[test]
    fn solo_los_cli_verificados_reciben_el_directorio_de_skills() {
        assert_eq!(skills_args("claude", "/tmp/proj"), vec!["--add-dir", "/tmp/proj"]);
        assert_eq!(skills_args("kimi", "/tmp/proj"), vec!["--add-dir", "/tmp/proj"]);
        assert_eq!(skills_args("grok", "/tmp/proj"), vec!["--add-dir", "/tmp/proj"]);

        // Codex mira en `.agents/skills` y toma rutas por skill, no un
        // directorio padre. Sin poder verificarlo contra una ejecución real, no
        // se le pasa nada: su terminal se comporta como hasta hoy.
        assert!(skills_args("codex", "/tmp/proj").is_empty());
        assert!(skills_args("gemini", "/tmp/proj").is_empty());
    }

    /// Codex separa "cuándo pregunta" de "qué puede escribir". Sin el segundo
    /// eje, yolo sería "no pregunta y tampoco puede hacer nada", que es la peor
    /// combinación posible.
    #[test]
    fn codex_necesita_los_dos_ejes_para_trabajar_sin_preguntar() {
        let yolo = permission_args("codex", Mode::Yolo);
        assert!(yolo.contains(&"--sandbox") && yolo.contains(&"danger-full-access"), "{yolo:?}");

        let plan = permission_args("codex", Mode::Plan);
        assert!(plan.contains(&"read-only"), "plan no puede escribir: {plan:?}");
    }

    /// Ningún modo puede quedarse sin banderas: un modo que no se traduce es un
    /// ajuste que la interfaz promete y el CLI ignora.
    #[test]
    fn ningun_modo_se_queda_sin_traducir() {
        for provider in ["claude", "codex", "kimi", "gemini", "grok"] {
            for mode in [Mode::Plan, Mode::AskEdits, Mode::AutoEdit, Mode::Yolo] {
                assert!(!permission_args(provider, mode).is_empty(), "{provider} en {mode:?} no traduce a nada");
            }
        }
    }

}
