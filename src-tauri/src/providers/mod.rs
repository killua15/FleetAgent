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
    let claude_like = matches!(provider, "claude" | "kimi");
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
        for provider in ["claude", "codex", "kimi", "gemini"] {
            for mode in [Mode::Plan, Mode::AskEdits, Mode::AutoEdit, Mode::Yolo] {
                assert!(!permission_args(provider, mode).is_empty(), "{provider} en {mode:?} no traduce a nada");
            }
        }
    }

}
