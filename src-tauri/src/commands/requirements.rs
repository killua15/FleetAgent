//! What Hangar needs on the machine, and what is missing.
//!
//! Hangar orchestrates other people's CLIs, so on a fresh machine most of it
//! is inert until those exist. Until now that showed up as failures at the
//! point of use — a terminal that won't open, a PR button that errors, an
//! install hint naming a tool the user doesn't have either. One screen that
//! answers "what do I still need" turns a series of dead ends into a list.
//!
//! Two rules this module exists to keep:
//!
//! **Name the step that is actually missing.** `uv tool install graphifyy` is
//! useless advice on a Mac with no `uv`, and worse than useless because it
//! looks actionable. Requirements declare what they need first, and the hint
//! that comes back is for the first unmet link in the chain.
//!
//! **Say what it buys.** A checklist of binaries is a chore; a list of what
//! each one unlocks is a decision the user can make. Most of these are
//! genuinely optional and Hangar should say so rather than nag.

use serde::Serialize;
use tauri::AppHandle;

/// How much of Hangar depends on this being present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Need {
    /// Nothing works without it.
    Required,
    /// A named feature is unavailable; everything else is fine.
    Optional,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Requirement {
    pub id: String,
    pub label: String,
    pub need: Need,
    /// What having it gives you, in the user's terms.
    pub unlocks: String,
    pub installed: bool,
    /// Absolute path, when found. Shown because "it works in my terminal" and
    /// "Hangar can find it" are different claims — a GUI app launched from
    /// Finder inherits a minimal PATH.
    pub path: Option<String>,
    /// The command to run **next**, which is the prerequisite's when this
    /// requirement's own prerequisite is missing. `None` once it is installed.
    pub install_hint: Option<String>,
    /// Set when the hint above is for a prerequisite rather than for this
    /// requirement itself, so the UI can say why it is being told about `uv`
    /// when it asked about a knowledge graph.
    pub blocked_by: Option<String>,
}

/// One tool Hangar looks for, and what has to exist before it can be installed.
struct Tool {
    id: &'static str,
    label: &'static str,
    binary: &'static str,
    need: Need,
    unlocks: &'static str,
    /// Ids of tools that must exist before `hint` can even be run.
    requires: &'static [&'static str],
    hint: &'static str,
}

/// Everything Hangar shells out to.
///
/// `claude` is the only hard requirement: Hangar is an orchestrator, and with
/// no CLI to orchestrate a terminal has nothing to open. The rest each turn
/// off exactly one feature, which is what `unlocks` has to say.
const TOOLS: &[Tool] = &[
    Tool {
        id: "claude",
        label: "Claude Code",
        binary: "claude",
        need: Need::Required,
        unlocks: "Terminals, headless turns and account rotation — Hangar's whole reason to exist.",
        requires: &[],
        hint: "npm install -g @anthropic-ai/claude-code",
    },
    Tool {
        id: "git",
        label: "git",
        binary: "git",
        need: Need::Required,
        unlocks: "Worktrees, so parallel agents never write the same file. Also every diff and commit view.",
        requires: &[],
        hint: "xcode-select --install",
    },
    Tool {
        id: "gh",
        label: "GitHub CLI",
        binary: "gh",
        need: Need::Optional,
        unlocks: "Opening pull requests, reading their comments, and installing skills from GitHub repos.",
        requires: &[],
        hint: "brew install gh   # then: gh auth login",
    },
    Tool {
        id: "codex",
        label: "Codex",
        binary: "codex",
        need: Need::Optional,
        unlocks: "Codex accounts alongside your Claude ones.",
        requires: &[],
        hint: "npm install -g @openai/codex",
    },
    Tool {
        id: "gemini",
        label: "Gemini CLI",
        binary: "gemini",
        need: Need::Optional,
        unlocks: "Gemini accounts alongside your Claude ones.",
        requires: &[],
        hint: "npm install -g @google/gemini-cli",
    },
    // The graph's chain, which is the whole reason this module exists. macOS
    // ships Python 3.9 and graphify needs 3.10+, so `uv` is not a convenience
    // here — it is the thing that brings a usable Python.
    Tool {
        id: "uv",
        label: "uv",
        binary: "uv",
        need: Need::Optional,
        unlocks: "Nothing on its own — it's what installs the code graph below.",
        requires: &[],
        hint: "brew install uv",
    },
    Tool {
        id: "graphify",
        label: "Code knowledge graph",
        binary: crate::graph::BINARY,
        need: Need::Optional,
        unlocks: "A map of your code that agents consult instead of re-deriving it with twenty greps every turn.",
        requires: &["uv"],
        // Two i's in the package, one in the binary. The [sql] extra is not
        // optional: without it, .sql files contribute nothing to the graph.
        hint: crate::graph::INSTALL_HINT,
    },
    Tool {
        id: "graphify-mcp",
        label: "Graph MCP server",
        binary: crate::graph::MCP_BINARY,
        need: Need::Optional,
        unlocks: "Serving that graph to agents. Without it the graph builds but never reaches anyone.",
        requires: &["uv"],
        hint: crate::graph::INSTALL_HINT,
    },
];

/// Resolves a binary to an absolute path, or `None` when it isn't installed.
///
/// `resolve_binary` falls back to the bare name so a PATH-based spawn still
/// works in dev, so a non-absolute answer means nothing was found on disk.
fn locate(binary: &str) -> Option<String> {
    let resolved = crate::providers::resolve_binary(binary);
    std::path::Path::new(&resolved).is_absolute().then_some(resolved)
}

/// Everything Hangar needs, with the *next* command for each missing one.
#[tauri::command]
pub fn list_requirements() -> Vec<Requirement> {
    let found: std::collections::BTreeMap<&str, Option<String>> =
        TOOLS.iter().map(|t| (t.id, locate(t.binary))).collect();

    TOOLS
        .iter()
        .map(|tool| {
            let path = found.get(tool.id).cloned().flatten();
            let installed = path.is_some();

            // The first unmet prerequisite decides what the user is told. This
            // is the whole point: a hint for a command they cannot run yet is
            // a dead end dressed as an instruction.
            let blocker = tool
                .requires
                .iter()
                .find(|id| found.get(*id).map(Option::is_none).unwrap_or(true))
                .and_then(|id| TOOLS.iter().find(|t| t.id == *id));

            let (install_hint, blocked_by) = match (installed, blocker) {
                (true, _) => (None, None),
                (false, Some(prereq)) => (Some(prereq.hint.to_string()), Some(prereq.label.to_string())),
                (false, None) => (Some(tool.hint.to_string()), None),
            };

            Requirement {
                id: tool.id.to_string(),
                label: tool.label.to_string(),
                need: tool.need,
                unlocks: tool.unlocks.to_string(),
                installed,
                path,
                install_hint,
                blocked_by,
            }
        })
        .collect()
}

/// Re-checks after the user installs something, without restarting the app.
///
/// `resolve_binary` caches in a process-wide mutex — deliberately, since it can
/// spawn a login shell and is called on every terminal open. That cache is
/// exactly what would make a freshly installed tool keep reading as missing
/// until relaunch, which reads as "I did what it said and nothing happened".
#[tauri::command]
pub fn recheck_requirements(_app: AppHandle) -> Vec<Requirement> {
    crate::providers::forget_resolved_binaries();
    list_requirements()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect this module exists to fix: on a clean Mac the graph's hint
    /// was `uv tool install …`, and `uv` isn't there either. The user runs it,
    /// gets `command not found`, and the app has lied to them.
    #[test]
    fn a_missing_prerequisite_replaces_the_hint_with_its_own() {
        let reqs = list_requirements();
        let graph = reqs.iter().find(|r| r.id == "graphify").expect("the graph must be listed");
        let uv = reqs.iter().find(|r| r.id == "uv").expect("uv must be listed");

        if !uv.installed && !graph.installed {
            assert_eq!(graph.blocked_by.as_deref(), Some("uv"), "the blocker has to be named");
            assert_eq!(
                graph.install_hint.as_deref(),
                Some("brew install uv"),
                "a hint the user cannot run yet is a dead end dressed as an instruction"
            );
        }
    }

    /// An installed tool has nothing left to tell the user to do.
    #[test]
    fn an_installed_tool_offers_no_hint() {
        for req in list_requirements().iter().filter(|r| r.installed) {
            assert!(req.install_hint.is_none(), "{} is installed but still suggests a command", req.id);
            assert!(req.path.is_some(), "{} is installed but reports no path", req.id);
        }
    }

    /// Hangar orchestrates CLIs; with none of them a terminal has nothing to
    /// open. Everything else has to stay optional, or the app starts nagging
    /// about tools the user has no use for.
    #[test]
    fn only_the_orchestrated_cli_and_git_are_required() {
        let required: Vec<String> =
            list_requirements().into_iter().filter(|r| r.need == Need::Required).map(|r| r.id).collect();

        assert_eq!(required, vec!["claude", "git"], "got: {required:?}");
    }

    /// Every tool says what it buys. A checklist of binaries is a chore; a
    /// list of what each unlocks is a decision.
    #[test]
    fn every_requirement_explains_what_it_unlocks() {
        for req in list_requirements() {
            assert!(!req.unlocks.trim().is_empty(), "{} says nothing about why it matters", req.id);
        }
    }
}
