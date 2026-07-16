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

/// Whether a provider's CLI is installed and resolvable on this machine.
/// Used by the Add-account wizard to gate the subscription-login step and
/// flag providers whose CLI is missing. `resolve_binary` returns an
/// absolute path when found and the bare name otherwise, so a resolved
/// value different from the bare name means it was located.
pub fn is_provider_available(provider: &str) -> bool {
    let binary = match provider {
        "claude" => claude::BINARY,
        "codex" => "codex",
        "gemini" => "gemini",
        _ => return false,
    };
    resolve_binary(binary) != binary
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_claude_to_an_absolute_executable_path() {
        let resolved = resolve_binary("claude");
        assert_ne!(resolved, "claude", "expected claude to be found on this dev machine");
        assert!(std::path::Path::new(&resolved).is_absolute());
        assert!(std::path::Path::new(&resolved).is_file());
    }

    #[test]
    fn unresolvable_binary_falls_back_to_bare_name() {
        assert_eq!(resolve_binary("definitely-not-a-real-cli-xyz"), "definitely-not-a-real-cli-xyz");
    }

    #[test]
    fn provider_availability_matches_installed_clis() {
        assert!(is_provider_available("claude"));
        assert!(!is_provider_available("codex"));
        assert!(!is_provider_available("gemini"));
        assert!(!is_provider_available("not-a-provider"));
    }
}
