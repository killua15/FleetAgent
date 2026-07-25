pub mod claude;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Read-only — proposes changes but never edits or executes. Fleet's
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

/// The CLI a provider is driven through, or `None` for a provider Fleet
/// doesn't know. Single source of truth so "is it installed?" and "what do I
/// run?" can never disagree about which binary a provider means.
pub fn provider_binary(provider: &str) -> Option<&'static str> {
    match provider {
        "claude" => Some(claude::BINARY),
        "codex" => Some("codex"),
        "gemini" => Some("gemini"),
        _ => None,
    }
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
        let dir = std::env::temp_dir().join(format!("fleet-test-path-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake_cli = dir.join("fleet-fake-cli");
        std::fs::write(&fake_cli, "#!/bin/sh\nexit 0\n").unwrap();

        let original_path = std::env::var("PATH").unwrap_or_default();
        std::env::set_var("PATH", format!("{}:{original_path}", dir.display()));

        let resolved = resolve_binary("fleet-fake-cli");
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
