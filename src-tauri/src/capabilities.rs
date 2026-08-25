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
    /// The same, on GitLab. A separate capability rather than a second binary
    /// on `PullRequests`: someone who only works on GitHub must never be told
    /// to install `glab`, and vice versa — which is the whole point of
    /// reporting a capability where it is used.
    MergeRequests,
    /// Install skills from GitHub repositories.
    SkillsFromGitHub,
    /// Build a code knowledge graph.
    CodeGraph,
    /// Serve that graph to agents over MCP.
    GraphToAgents,
    /// Codex accounts, which need their own CLI.
    CodexAccounts,
    /// Gemini accounts, same.
    GeminiAccounts,
    /// Grok accounts, same.
    GrokAccounts,
    /// opencode accounts, which need its own CLI. Isolation is by
    /// `XDG_DATA_HOME` rather than a dedicated variable — see `providers::spec`.
    OpencodeAccounts,
    // Kimi is deliberately absent: it runs through the `claude` binary with
    // Moonshot's endpoint in the environment (`providers::spec`), so there is
    // nothing to install. What it needs is an account, and a row telling
    // someone to install a CLI that doesn't exist would be worse than silence.
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

/// How to get a binary, in the shapes a real machine needs.
///
/// One command was not enough. `brew install uv` is not executable on a Mac
/// without Homebrew — the same dead end this module exists to stop, one level
/// down — so the route that is actually runnable has to be chosen, not assumed.
#[derive(Clone, Copy)]
pub struct Install {
    /// Preferred when Homebrew is there.
    pub brew: Option<&'static str>,
    /// Works without it.
    pub fallback: &'static str,
    /// A step that is **not** a command. Installing a provider's CLI doesn't
    /// give you an account, and adding one is Hangar's own wizard, not a shell
    /// line — so pointing at a command there would leave the user stuck one
    /// step from done.
    pub then: Option<&'static str>,
}

impl Install {
    /// The command to show: the one this machine can actually run.
    pub fn command(&self) -> &'static str {
        match self.brew {
            Some(brew) if locate("brew").is_some() => brew,
            _ => self.fallback,
        }
    }

    /// The other route, when there is one worth showing under the first.
    pub fn alternative(&self) -> Option<&'static str> {
        let shown = self.command();
        self.brew.filter(|b| *b != shown).or(Some(self.fallback)).filter(|other| *other != shown)
    }
}

/// A binary this machine needs, and how to get it.
struct Binary {
    name: &'static str,
    install: Install,
    /// Must exist before the install can even be typed. macOS ships Python 3.9
    /// and graphify needs 3.10+, so `uv` isn't a convenience — it's the thing
    /// that brings a usable Python, and telling someone to run
    /// `uv tool install` when they have no `uv` is a dead end dressed as an
    /// instruction.
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
    /// Which forge CLI also has to be logged in, if any. Installed-but-not-
    /// authenticated fails exactly as hard as absent, so the two live together.
    needs_login: NeedsLogin,
}

/// Which CLI's login a capability depends on.
///
/// Was a bare `needs_gh_auth: bool` while GitHub was the only forge. A second
/// one made the boolean lie: `glab` being logged in says nothing about `gh`,
/// and a machine can perfectly well have one and not the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NeedsLogin {
    None,
    Gh,
    /// Checked against gitlab.com. A self-hosted instance is authenticated per
    /// host and can only be asked about once a repo is known, so that check
    /// lives on the real call path in `forges::gitlab`; this row exists to
    /// answer the settings screen's "is this set up?", not to gate a specific
    /// project.
    Glab,
}

const GIT: Binary = Binary {
    name: "git",
    install: Install {
        brew: Some("brew install git"),
        fallback: "xcode-select --install",
        then: None,
    },
    prerequisite: None,
};
const GH: Binary = Binary {
    name: "gh",
    install: Install {
        brew: Some("brew install gh"),
        fallback: "See https://cli.github.com for your platform's installer",
        then: Some("Then run `gh auth login` — installed isn't the same as logged in, and Hangar needs both."),
    },
    prerequisite: None,
};
const GLAB: Binary = Binary {
    name: "glab",
    install: Install {
        brew: Some("brew install glab"),
        fallback: "See https://gitlab.com/gitlab-org/cli for your platform's installer",
        then: Some("Then run `glab auth login` — once per instance, so a self-hosted GitLab needs its own."),
    },
    prerequisite: None,
};
const UV: Binary = Binary {
    name: "uv",
    install: Install {
        brew: Some("brew install uv"),
        fallback: "curl -LsSf https://astral.sh/uv/install.sh | sh",
        then: None,
    },
    prerequisite: None,
};

/// Every provider CLI installs the same way and needs the same step after, so
/// the shape is shared and only the package changes.
///
/// `const` items rather than a helper call inside the array: an rvalue built by
/// a function call isn't promoted to `'static`, and these have to outlive the
/// `spec()` that hands them out.
const fn provider(name: &'static str, npm_package: &'static str) -> Binary {
    Binary {
        name,
        install: Install {
            brew: None,
            fallback: npm_package,
            then: Some("Then add an account in Settings → accounts. The CLI on its own signs nobody in."),
        },
        prerequisite: None,
    }
}

const CLAUDE: Binary = provider(crate::providers::claude::BINARY, "npm install -g @anthropic-ai/claude-code");
const CODEX: Binary = provider("codex", "npm install -g @openai/codex");
const GEMINI: Binary = provider("gemini", "npm install -g @google/gemini-cli");
const GROK: Binary = provider("grok", "npm install -g @vibe-kit/grok-cli");
const OPENCODE: Binary = provider("opencode", "brew install opencode");

const GRAPHIFY: Binary = Binary {
    name: crate::graph::BINARY,
    install: Install { brew: None, fallback: crate::graph::INSTALL_HINT, then: None },
    prerequisite: Some("uv"),
};
const GRAPHIFY_MCP: Binary = Binary {
    name: crate::graph::MCP_BINARY,
    install: Install { brew: None, fallback: crate::graph::INSTALL_HINT, then: None },
    prerequisite: Some("uv"),
};

impl Capability {
    pub const ALL: &'static [Capability] = &[
        Capability::RunAgents,
        Capability::GitWork,
        Capability::PullRequests,
        Capability::MergeRequests,
        Capability::SkillsFromGitHub,
        Capability::CodeGraph,
        Capability::GraphToAgents,
        Capability::CodexAccounts,
        Capability::GeminiAccounts,
        Capability::GrokAccounts,
        Capability::OpencodeAccounts,
    ];

    pub fn spec(self) -> CapabilitySpec {
        match self {
            Capability::RunAgents => CapabilitySpec {
                label: "Run agents",
                enables: "Terminals, headless turns and account rotation — Hangar's whole reason to exist.",
                essential: true,
                binaries: &[CLAUDE],
                needs_login: NeedsLogin::None,
            },
            Capability::GitWork => CapabilitySpec {
                label: "Work on repositories",
                enables: "Worktrees, so parallel agents never write the same file. Also every diff, branch and commit view.",
                essential: true,
                binaries: &[GIT],
                needs_login: NeedsLogin::None,
            },
            Capability::PullRequests => CapabilitySpec {
                label: "Pull requests",
                enables: "Opening PRs from a session and reading their review comments.",
                essential: false,
                binaries: &[GIT, GH],
                needs_login: NeedsLogin::Gh,
            },
            Capability::MergeRequests => CapabilitySpec {
                label: "Merge requests",
                enables: "Opening merge requests from a session and reading their review comments, on GitLab.",
                essential: false,
                binaries: &[GIT, GLAB],
                needs_login: NeedsLogin::Glab,
            },
            Capability::SkillsFromGitHub => CapabilitySpec {
                label: "Skills from GitHub",
                enables: "Installing skills straight from a repository, private ones included.",
                essential: false,
                binaries: &[GH],
                needs_login: NeedsLogin::Gh,
            },
            Capability::CodeGraph => CapabilitySpec {
                label: "Code knowledge graph",
                enables: "A map of your code that agents consult instead of re-deriving it with twenty greps every turn.",
                essential: false,
                binaries: &[UV, GRAPHIFY],
                needs_login: NeedsLogin::None,
            },
            Capability::CodexAccounts => CapabilitySpec {
                label: "Codex accounts",
                enables: "Running Codex agents alongside your Claude ones, with the same isolation and rotation.",
                essential: false,
                binaries: &[CODEX],
                needs_login: NeedsLogin::None,
            },
            Capability::OpencodeAccounts => CapabilitySpec {
                label: "opencode accounts",
                enables: "Running opencode agents alongside your Claude ones, each isolated to its own credentials.",
                essential: false,
                binaries: &[OPENCODE],
                needs_login: NeedsLogin::None,
            },
            Capability::GeminiAccounts => CapabilitySpec {
                label: "Gemini accounts",
                enables: "Running Gemini agents alongside your Claude ones.",
                essential: false,
                binaries: &[GEMINI],
                needs_login: NeedsLogin::None,
            },
            Capability::GrokAccounts => CapabilitySpec {
                label: "Grok accounts",
                enables: "Running Grok agents alongside your Claude ones.",
                essential: false,
                binaries: &[GROK],
                needs_login: NeedsLogin::None,
            },
            Capability::GraphToAgents => CapabilitySpec {
                label: "Graph served to agents",
                enables: "Handing that graph to agents over MCP. Without it the graph builds but never reaches anyone.",
                essential: false,
                binaries: &[UV, GRAPHIFY_MCP],
                needs_login: NeedsLogin::None,
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
        let (logged_in, reason, hint) = match self.spec().needs_login {
            NeedsLogin::None => return Ok(()),
            NeedsLogin::Gh => (gh_is_authenticated(), "the GitHub CLI isn't logged in", "gh auth login"),
            NeedsLogin::Glab => (crate::forges::gitlab::is_authenticated("gitlab.com"), "the GitLab CLI isn't logged in", "glab auth login"),
        };
        if !logged_in {
            return Err(Blocked { capability: self, reason: reason.to_string(), hint: Some(hint.to_string()) });
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
                        .map(|b| b.install.command())
                        .unwrap_or(prereq);
                    (prereq, prereq_hint)
                }
                _ => (binary.name, binary.install.command()),
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
    install_for(binary).map(|i| i.command())
}

/// The full route for a binary, for the install guide that opens under a
/// blocked row.
pub fn install_for(binary: &str) -> Option<Install> {
    Capability::ALL.iter().flat_map(|c| c.spec().binaries).find(|b| b.name == binary).map(|b| b.install)
}

/// Whether that binary needs another one first.
pub fn prerequisite_of(binary: &str) -> Option<&'static str> {
    Capability::ALL.iter().flat_map(|c| c.spec().binaries).find(|b| b.name == binary).and_then(|b| b.prerequisite)
}

fn gh_is_authenticated() -> bool {
    crate::forges::github::gh_output(None, &["auth", "status"]).is_ok_and(|o| o.status.success())
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

    /// The GitLab row must ask for `glab` and never for `gh`. Someone who
    /// works only on GitLab being told to install the GitHub CLI is exactly
    /// the nagging this module's doc comment rules out — and the reverse
    /// matters just as much, which is why these are two capabilities and not
    /// one with four binaries.
    #[test]
    fn merge_requests_need_glab_and_never_gh() {
        let binaries = Capability::MergeRequests.binaries();

        assert!(binaries.contains(&"git"), "got: {binaries:?}");
        assert!(binaries.contains(&"glab"), "got: {binaries:?}");
        assert!(!binaries.contains(&"gh"), "a GitLab-only machine must never be asked for the GitHub CLI: {binaries:?}");
        assert!(!Capability::PullRequests.binaries().contains(&"glab"), "and the reverse: got {:?}", Capability::PullRequests.binaries());
    }

    /// The login hint has to name the CLI that is actually logged out. Both
    /// forges reaching for `gh auth login` was the concrete failure the
    /// `needs_gh_auth` boolean would have caused.
    #[test]
    fn a_logged_out_forge_is_told_to_log_into_its_own_cli() {
        let blocked = Blocked {
            capability: Capability::MergeRequests,
            reason: "the GitLab CLI isn't logged in".to_string(),
            hint: Some("glab auth login".to_string()),
        };

        assert_eq!(blocked.message(), "the GitLab CLI isn't logged in — run `glab auth login`");
    }

    /// Every provider with its own CLI has to be listed, or the panel says
    /// "everything is fine" on a machine that can't run Codex. Dropping Codex
    /// and Gemini is exactly what the tool-shaped version did when it became
    /// capability-shaped.
    #[test]
    fn every_provider_with_its_own_binary_is_declared() {
        let declared: Vec<&str> = Capability::ALL.iter().flat_map(|c| c.binaries()).collect();

        for provider in ["claude", "codex", "gemini", "grok"] {
            let binary = crate::providers::spec(provider).expect("known provider").binary;
            assert!(declared.contains(&binary), "{provider} runs `{binary}`, which nothing declares: {declared:?}");
        }
    }

    /// Kimi runs *through* the `claude` binary with Moonshot's endpoint in the
    /// environment. A row telling someone to install a `kimi` CLI would send
    /// them looking for something that does not exist.
    #[test]
    fn kimi_gets_no_row_because_it_has_no_binary_of_its_own() {
        let kimi = crate::providers::spec("kimi").expect("kimi is a known provider");

        assert_eq!(kimi.binary, crate::providers::claude::BINARY, "if this changes, Kimi needs its own capability");
        assert!(
            !Capability::ALL.iter().any(|c| c.spec().label.contains("Kimi")),
            "Kimi needs an account, not an install"
        );
    }

    /// The Homebrew assumption, one level down from the one this module fixed:
    /// `brew install uv` is not executable on a Mac without Homebrew.
    #[test]
    fn a_tool_with_no_homebrew_route_still_offers_one_that_runs() {
        let uv = install_for("uv").expect("uv is declared");

        assert!(!uv.command().is_empty());
        assert!(uv.alternative().is_some(), "there has to be a route for a machine with no brew");
        assert_ne!(uv.command(), uv.alternative().unwrap(), "showing the same command twice helps nobody");
    }

    /// Installing a provider's CLI does not sign anyone in, and the account
    /// wizard is Hangar's own — not a shell command. A guide that stops at the
    /// install leaves the user one step short and thinking they are done.
    #[test]
    fn a_provider_guide_says_the_step_that_isnt_a_command() {
        let codex = install_for("codex").expect("codex is declared");

        assert!(codex.then.is_some_and(|t| t.contains("account")), "the guide has to name the account step");
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
