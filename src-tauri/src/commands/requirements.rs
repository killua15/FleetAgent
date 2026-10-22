//! What Hangar needs on the machine — **derived** from what features declare.
//!
//! This module used to keep its own table of tools with hand-written prose
//! about what each unlocked. That was a second source of truth: nothing tied
//! the prose to the code, so deleting a feature left the app still advertising
//! it, and adding one that shelled out to something new went unnoticed.
//!
//! Now the capabilities own the declaration (`crate::capabilities`) and this is
//! a view over them. A tool appears here *because* some capability names it,
//! and what it unlocks is that capability's own words — the same sentence the
//! feature shows when it refuses.

use crate::capabilities::{self, Capability};
use serde::Serialize;
use tauri::AppHandle;

/// A capability and whether it can run, for the settings screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityStatus {
    pub id: Capability,
    pub label: String,
    /// What the user gets. The capability's own words.
    pub enables: String,
    /// Whether Hangar is useless without it.
    pub essential: bool,
    pub available: bool,
    /// One sentence naming the missing step, when unavailable.
    pub reason: Option<String>,
    pub install_hint: Option<String>,
    /// The tools this capability rests on, so the user can see *why* it needs
    /// what it needs rather than being handed a bare command.
    pub tools: Vec<ToolStatus>,
    /// Where to connect what's missing, for a row that rests on a service
    /// rather than a tool — a button, not a command.
    pub setup: Option<capabilities::Setup>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub name: String,
    pub installed: bool,
    /// Absolute path when found. Shown because "it works in my terminal" and
    /// "Hangar can find it" are different claims for an app launched from
    /// Finder with a minimal PATH — and that gap is most of the confusion when
    /// a CLI is present but unusable.
    pub path: Option<String>,
    pub install_hint: Option<String>,
    /// The other route, when the one above isn't the only way. Homebrew is not
    /// on every Mac, and a command the user can't run is the dead end this
    /// whole module exists to stop.
    pub alternative: Option<String>,
    /// A step that isn't a command — logging in, or adding an account. The
    /// install guide is incomplete without it: a provider CLI signs nobody in.
    pub then: Option<String>,
    /// Named when this tool needs another one first, so the UI can explain why
    /// it is talking about `uv` when the user asked about a knowledge graph.
    pub blocked_by: Option<String>,
}

/// Every capability, with what is missing.
///
/// `check_offline` rather than `check`: this renders a screen, and spawning
/// `gh auth status` per row would make opening Settings wait on a network
/// call. Login state is reported by the feature that needs it, at the moment
/// it needs it.
#[tauri::command]
pub fn list_requirements() -> Vec<CapabilityStatus> {
    Capability::ALL
        .iter()
        .map(|cap| {
            let spec = cap.spec();
            let blocked = cap.check_offline().err();
            CapabilityStatus {
                id: *cap,
                label: spec.label.to_string(),
                enables: spec.enables.to_string(),
                essential: spec.essential,
                available: blocked.is_none(),
                reason: blocked.as_ref().map(|b| b.reason.clone()),
                install_hint: blocked.as_ref().and_then(|b| b.hint.clone()),
                tools: cap.binaries().into_iter().map(tool_status).collect(),
                setup: blocked.as_ref().and(cap.service()).map(|s| s.setup()),
            }
        })
        .collect()
}

fn tool_status(name: &'static str) -> ToolStatus {
    let path = capabilities::locate(name);
    let installed = path.is_some();
    // The first unmet link decides what is shown. A hint for a command the
    // user cannot run yet is a dead end dressed as an instruction.
    let blocker = capabilities::prerequisite_of(name).filter(|p| capabilities::locate(p).is_none());

    // When a prerequisite is missing, the guide shown is *its* guide: that is
    // the step the user can actually take.
    let install = blocker.and_then(capabilities::install_for).or_else(|| capabilities::install_for(name));

    ToolStatus {
        name: name.to_string(),
        installed,
        install_hint: (!installed).then(|| install.map(|i| i.command().to_string())).flatten(),
        alternative: (!installed).then(|| install.and_then(|i| i.alternative()).map(str::to_string)).flatten(),
        then: (!installed).then(|| install.and_then(|i| i.then).map(str::to_string)).flatten(),
        blocked_by: (!installed).then_some(blocker).flatten().map(str::to_string),
        path,
    }
}

/// Re-checks after the user installs something, without restarting the app.
///
/// `resolve_binary` caches in a process-wide mutex — deliberately, since it can
/// spawn a login shell and runs on every terminal open. That cache is exactly
/// what would make a freshly installed tool keep reading as missing until
/// relaunch, which reads as "I did what it said and nothing happened".
#[tauri::command]
pub fn recheck_requirements(_app: AppHandle) -> Vec<CapabilityStatus> {
    crate::providers::forget_resolved_binaries();
    list_requirements()
}

/// Whether one capability can run right now, login included.
///
/// This is the call a screen makes to disable a button *and say why* — the
/// same sentence the command would have returned had the user pressed it.
#[tauri::command]
pub fn check_capability(capability: Capability) -> Result<(), String> {
    capability.check().map_err(|b| b.message())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The panel is a view, not a second list: everything it shows has to come
    /// from a capability that declared it.
    #[test]
    fn every_row_comes_from_a_declared_capability() {
        let rows = list_requirements();

        assert_eq!(rows.len(), Capability::ALL.len());
        for row in &rows {
            // A row with no tools has to rest on a service instead, or nothing
            // could ever block it.
            assert!(!row.tools.is_empty() || row.id.service().is_some(), "{} shows no tools and no service — then nothing can block it", row.label);
            assert!(!row.enables.trim().is_empty(), "{} says nothing about why it matters", row.label);
        }
    }

    /// An available capability has nothing left to tell the user to do, and an
    /// unavailable one always names a step. A row with neither is a dead end.
    #[test]
    fn a_row_either_works_or_names_the_next_step() {
        for row in list_requirements() {
            if row.available {
                assert!(row.reason.is_none(), "{} is available but still complains", row.label);
                assert!(row.tools.iter().all(|t| t.installed), "{} is available with a missing tool", row.label);
            } else {
                assert!(row.reason.is_some(), "{} is blocked and won't say why", row.label);
                // And a blocked service row says where to go, since there is no
                // command to run.
                if row.id.service().is_some() {
                    assert!(row.setup.is_some(), "{} needs connecting and doesn't say where", row.label);
                }
            }
        }
    }

    /// An installed tool reports where Hangar found it, which is the answer to
    /// "but it works in my terminal".
    #[test]
    fn an_installed_tool_reports_its_path_and_no_hint() {
        for tool in list_requirements().into_iter().flat_map(|r| r.tools).filter(|t| t.installed) {
            assert!(tool.path.is_some(), "{} is installed but reports no path", tool.name);
            assert!(tool.install_hint.is_none(), "{} is installed but still suggests a command", tool.name);
        }
    }
}
