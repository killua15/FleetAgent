//! What each feature needs to work, declared once.
//!
//! Hangar shells out to other people's CLIs, so most features are one missing
//! binary away from not working. Before this, each decided for itself whether
//! to check: `gh` had `gh_available_and_authed`, providers had
//! `is_provider_available`, and **`git` — ten call sites, the most essential
//! tool of all — had nothing**. A Mac without the Xcode command line tools
//! didn't say "install the developer tools", it said
//! `git rev-parse HEAD: No such file or directory`.
//!
//! So the unit here is the **capability**, not the tool. A capability is a
//! thing the user wants to do; it declares what has to be true first, and
//! answers with either "yes" or one sentence naming the step that is missing.
//! Both the feature and the settings screen ask the same question and get the
//! same sentence — which is the point. The old requirements table listed tools
//! and hand-wrote prose about what each unlocked; nothing tied that prose to
//! the code, so deleting a feature left the app still advertising it.
//!
//! Two things deliberately *not* modelled here:
//!
//! - **State that depends on arguments** — whether *this* project has a git
//!   remote, whether *this* graph is built. That needs the argument, so it
//!   stays at the call site. This layer answers only what is true of the
//!   machine.
//! - **Nagging.** A capability is reported where it is used, not everywhere.
//!   Someone who only wants a terminal should never hear about `gh`.

use serde::Serialize;

/// Something the user can do, and therefore something that can be unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    /// Run agents at all: terminals, headless turns, rotation.
    RunAgents,
    /// Anything touching a repo: worktrees, diffs, commits, branches.
    GitWork,
    /// Open pull requests and read their comments.
    PullRequests,
    /// Install skills from GitHub repositories.
    SkillsFromGitHub,
    /// Build a code knowledge graph.
    CodeGraph,
    /// Serve that graph to agents over MCP.
    GraphToAgents,
}

/// Why a capability is unavailable, in the shape both a disabled button and a
/// settings row can render.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocked {
    /// The capability that can't run.
    pub capability: Capability,
    /// One sentence, addressed to the user.
    pub reason: String,
    /// The command to run next, when there is one. `None` when the fix isn't a
    /// command (an expired login, say).
    pub hint: Option<String>,
}

impl Blocked {
    /// The message a command returns when it refuses. One string, so the same
    /// wording reaches a toast, a log line and a disabled tooltip.
    pub fn message(&self) -> String {
        match &self.hint {
            Some(hint) => format!("{} — run `{hint}`", self.reason),
            None => self.reason.clone(),
        }
    }
}

/// A binary this machine needs, and what to run to get it.
struct Binary {
    name: &'static str,
    /// What to run, assuming its own prerequisites are met.
    hint: &'static str,
    /// Must exist before `hint` can even be typed. macOS ships Python 3.9 and
    /// graphify needs 3.10+, so `uv` isn't a convenience — it's the thing that
    /// brings a usable Python, and telling someone to run `uv tool install`
    /// when they have no `uv` is a dead end dressed as an instruction.
    prerequisite: Option<&'static str>,
}

pub struct CapabilitySpec {
    pub label: &'static str,
    /// What the user gets. Written from their side of the screen, since this
    /// is what the settings screen shows.
    pub enables: &'static str,
    /// Without this, Hangar has nothing to offer at all.
    pub essential: bool,
    binaries: &'static [Binary],
    /// Whether `gh` also has to be logged in. Installed-but-not-authenticated
    /// fails exactly as hard as absent, so the two live together.
    needs_gh_auth: bool,
}

const GIT: Binary = Binary { name: "git", hint: "xcode-select --install", prerequisite: None };
const GH: Binary = Binary { name: "gh", hint: "brew install gh", prerequisite: None };
const UV: Binary = Binary { name: "uv", hint: "brew install uv", prerequisite: None };

impl Capability {
    pub const ALL: &'static [Capability] = &[
        Capability::RunAgents,
        Capability::GitWork,
        Capability::PullRequests,
        Capability::SkillsFromGitHub,
        Capability::CodeGraph,
        Capability::GraphToAgents,
    ];

    pub fn spec(self) -> CapabilitySpec {
        match self {
            Capability::RunAgents => CapabilitySpec {
                label: "Run agents",
                enables: "Terminals, headless turns and account rotation — Hangar's whole reason to exist.",
                essential: true,
                binaries: &[Binary {
                    name: crate::providers::claude::BINARY,
                    hint: "npm install -g @anthropic-ai/claude-code",
                    prerequisite: None,
                }],
                needs_gh_auth: false,
            },
            Capability::GitWork => CapabilitySpec {
                label: "Work on repositories",
                enables: "Worktrees, so parallel agents never write the same file. Also every diff, branch and commit view.",
                essential: true,
                binaries: &[GIT],
                needs_gh_auth: false,
            },
            Capability::PullRequests => CapabilitySpec {
                label: "Pull requests",
                enables: "Opening PRs from a session and reading their review comments.",
                essential: false,
                binaries: &[GIT, GH],
                needs_gh_auth: true,
            },
            Capability::SkillsFromGitHub => CapabilitySpec {
                label: "Skills from GitHub",
                enables: "Installing skills straight from a repository, private ones included.",
                essential: false,
                binaries: &[GH],
                needs_gh_auth: true,
            },
            Capability::CodeGraph => CapabilitySpec {
                label: "Code knowledge graph",
                enables: "A map of your code that agents consult instead of re-deriving it with twenty greps every turn.",
                essential: false,
                binaries: &[
                    UV,
                    Binary {
                        name: crate::graph::BINARY,
                        hint: crate::graph::INSTALL_HINT,
                        prerequisite: Some("uv"),
                    },
                ],
                needs_gh_auth: false,
            },
            Capability::GraphToAgents => CapabilitySpec {
                label: "Graph served to agents",
                enables: "Handing that graph to agents over MCP. Without it the graph builds but never reaches anyone.",
                essential: false,
                binaries: &[
                    UV,
                    Binary {
                        name: crate::graph::MCP_BINARY,
                        hint: crate::graph::INSTALL_HINT,
                        prerequisite: Some("uv"),
                    },
                ],
                needs_gh_auth: false,
            },
        }
    }

    /// Whether this can run right now, and if not, the next step.
    ///
    /// Cheap enough to call on every entry: binary resolution is cached in a
    /// process-wide map, so the common case is a lookup. The one exception is
    /// `gh auth status`, which spawns — hence [`check_offline`] for the paths
    /// that only care whether the tool exists.
    pub fn check(self) -> Result<(), Blocked> {
        self.check_offline()?;
        if self.spec().needs_gh_auth && !gh_is_authenticated() {
            return Err(Blocked {
                capability: self,
                reason: "the GitHub CLI isn't logged in".to_string(),
                hint: Some("gh auth login".to_string()),
            });
        }
        Ok(())
    }

    /// The binaries only — no process spawned.
    pub fn check_offline(self) -> Result<(), Blocked> {
        for binary in self.spec().binaries {
            if locate(binary.name).is_some() {
                continue;
            }
            // If what it needs first is *also* missing, that is the step to
            // name. Telling someone to run a command they can't run yet is the
            // failure this whole module exists to stop.
            let (missing, hint) = match binary.prerequisite {
                Some(prereq) if locate(prereq).is_none() => {
                    let prereq_hint = Capability::ALL
                        .iter()
                        .flat_map(|c| c.spec().binaries)
                        .find(|b| b.name == prereq)
                        .map(|b| b.hint)
                        .unwrap_or(prereq);
                    (prereq, prereq_hint)
                }
                _ => (binary.name, binary.hint),
            };
            return Err(Blocked {
                capability: self,
                reason: format!("`{missing}` isn't installed on this machine"),
                hint: Some(hint.to_string()),
            });
        }
        Ok(())
    }

    /// Every binary this capability names, prerequisites included. Used by the
    /// settings screen so the tool list is *derived* from what features declare
    /// rather than maintained beside it.
    pub fn binaries(self) -> Vec<&'static str> {
        self.spec().binaries.iter().map(|b| b.name).collect()
    }
}

/// Resolves a binary to an absolute path, or `None` when it isn't installed.
///
/// `resolve_binary` falls back to the bare name so a PATH-based spawn still
/// works in dev, so anything not absolute means nothing was found on disk.
pub fn locate(binary: &str) -> Option<String> {
    let resolved = crate::providers::resolve_binary(binary);
    std::path::Path::new(&resolved).is_absolute().then_some(resolved)
}

/// What to run to get a binary, for the settings screen's per-tool rows.
pub fn hint_for(binary: &str) -> Option<&'static str> {
    Capability::ALL.iter().flat_map(|c| c.spec().binaries).find(|b| b.name == binary).map(|b| b.hint)
}

/// Whether that binary needs another one first.
pub fn prerequisite_of(binary: &str) -> Option<&'static str> {
    Capability::ALL.iter().flat_map(|c| c.spec().binaries).find(|b| b.name == binary).and_then(|b| b.prerequisite)
}

fn gh_is_authenticated() -> bool {
    crate::commands::pull_requests::gh_output(None, &["auth", "status"]).is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The defect this replaced: the graph's hint was `uv tool install …` on a
    /// machine with no `uv`. The user runs it, gets `command not found`, and
    /// the app has lied to them.
    #[test]
    fn a_missing_prerequisite_is_what_gets_named() {
        if locate("uv").is_some() {
            return; // this machine can't demonstrate it
        }
        let blocked = Capability::CodeGraph.check_offline().expect_err("no uv means no graph");

        assert!(blocked.reason.contains("uv"), "the blocker has to be named, not the thing it blocks: {}", blocked.reason);
        assert_eq!(blocked.hint.as_deref(), Some("brew install uv"));
    }

    /// A refusal has to be readable on its own — it reaches a toast with no
    /// surrounding explanation.
    #[test]
    fn a_refusal_reads_as_a_sentence() {
        let blocked = Blocked {
            capability: Capability::PullRequests,
            reason: "the GitHub CLI isn't logged in".to_string(),
            hint: Some("gh auth login".to_string()),
        };

        assert_eq!(blocked.message(), "the GitHub CLI isn't logged in — run `gh auth login`");
    }

    /// Hangar orchestrates CLIs and works on repos; everything else turns off
    /// one feature. An app that nags about tools you have no use for gets
    /// ignored, so this list staying short is a feature.
    #[test]
    fn only_agents_and_git_are_essential() {
        let essential: Vec<&str> =
            Capability::ALL.iter().filter(|c| c.spec().essential).map(|c| c.spec().label).collect();

        assert_eq!(essential, vec!["Run agents", "Work on repositories"], "got: {essential:?}");
    }

    /// Every capability says what it buys, since that text is what the user
    /// reads when deciding whether to install anything.
    #[test]
    fn every_capability_explains_itself() {
        for cap in Capability::ALL {
            let spec = cap.spec();
            assert!(!spec.enables.trim().is_empty(), "{:?} says nothing about why it matters", cap);
            assert!(!spec.binaries.is_empty(), "{:?} declares no binaries — then it cannot be blocked", cap);
        }
    }

    /// Opening a PR needs the repo as much as the API: `gh pr create` runs
    /// against a checkout. Declaring only `gh` would let a machine with no git
    /// reach a very confusing failure.
    #[test]
    fn pull_requests_need_git_as_well_as_gh() {
        let binaries = Capability::PullRequests.binaries();

        assert!(binaries.contains(&"git"), "got: {binaries:?}");
        assert!(binaries.contains(&"gh"), "got: {binaries:?}");
    }

    /// git is required by more than one capability and is the tool that had no
    /// check at all before this existed.
    #[test]
    fn git_is_declared_by_the_capabilities_that_shell_out_to_it() {
        let users: Vec<&str> = Capability::ALL
            .iter()
            .filter(|c| c.binaries().contains(&"git"))
            .map(|c| c.spec().label)
            .collect();

        assert!(users.contains(&"Work on repositories"), "got: {users:?}");
        assert!(users.contains(&"Pull requests"), "got: {users:?}");
    }
}
