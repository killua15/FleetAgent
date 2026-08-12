//! Where a change request comes from — GitHub's pull requests, GitLab's merge
//! requests — and the one shape the rest of the app sees either as.
//!
//! **The seam is [`RepoPrSnapshot`] and the types under it**, and like
//! `task_sources`' [`Task`](crate::task_sources::Task) it is deliberately
//! small: a number, a state, two branch names, a flat list of comments. Draft
//! status, approval rules, pipelines and labels stay out on purpose — the
//! moment one forge's concept enters the model, every other adapter has to
//! fake it.
//!
//! This is not shaped like [`ProviderSpec`](crate::providers::ProviderSpec),
//! and the difference is worth naming. `ProviderSpec` is pure data because
//! every provider CLI does the *same thing* with different flags. GitHub and
//! GitLab run structurally different algorithms for the same question: one
//! GraphQL round trip carrying comments and a server-side
//! `review-requested:@me` search, versus a REST list whose reviewers are
//! classified locally and whose comments cost a call per merge request. No
//! `&'static [&'static str]` expresses that, so each forge is a module and the
//! differences live behind the dispatch functions at the bottom of this file —
//! one two-armed `match` each, in one place, instead of seven scattered
//! through `commands::pull_requests`. What genuinely *is* declarative turned
//! out to be three words on [`Forge`] itself — a struct's worth of ceremony
//! for no gain.

pub mod github;
pub mod gitlab;

use std::collections::HashSet;
use std::path::Path;

/// Which hosting product a repository lives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forge {
    Github,
    Gitlab,
}

impl Forge {
    /// The value stored in `pull_requests.forge`.
    pub fn as_str(self) -> &'static str {
        match self {
            Forge::Github => "github",
            Forge::Gitlab => "gitlab",
        }
    }

    /// Reads the column back. An unrecognised value means a row written before
    /// the column existed, and every one of those is a github.com pull request
    /// — that is what the column's `DEFAULT` encodes and this agrees with it.
    pub fn from_column(value: &str) -> Forge {
        match value {
            "gitlab" => Forge::Gitlab,
            _ => Forge::Github,
        }
    }

    /// Which forge a hostname belongs to, or `None` when nothing here can say.
    ///
    /// `github.com` and `gitlab.com` are exact. Anything whose host contains
    /// `gitlab` is taken to be a self-hosted GitLab, which covers
    /// `gitlab.acme.com` — the way very nearly every self-hosted instance is
    /// actually named.
    ///
    /// A host that says nothing about itself (`git.acme.com`) falls through to
    /// asking `glab`, on the reasoning that a host someone has logged into
    /// with `glab` **is** a GitLab host. A GitHub Enterprise instance on a
    /// vanity domain is not detected at all, and returning `None` is the
    /// honest answer: the project is skipped rather than pointed at a server
    /// that isn't GitHub.
    ///
    /// **A declaration beats all of that.** Everything below the registry check
    /// is a guess about someone else's server, and the user knows. That is also
    /// what makes [`DeclaredKind::GitOnly`] worth having: a plain git host
    /// (Gitea, a bare repo over ssh) otherwise pays a `glab auth status` spawn on
    /// every poll pass, forever, to fail exactly the same way.
    pub fn for_host(host: &str) -> Option<Forge> {
        let host = host.to_ascii_lowercase();
        if let Some(declared) = declared_kind(&host) {
            return declared.forge();
        }
        if host == "github.com" {
            return Some(Forge::Github);
        }
        if host == "gitlab.com" || host.contains("gitlab") {
            return Some(Forge::Gitlab);
        }
        gitlab::is_authenticated(&host).then_some(Forge::Gitlab)
    }

    /// What to call this forge in a sentence the user reads.
    pub fn display_name(self) -> &'static str {
        match self {
            Forge::Github => "GitHub",
            Forge::Gitlab => "GitLab",
        }
    }

    /// What this forge calls a change request. Notifications are read out of
    /// context, so calling a merge request a pull request there reads as a bug
    /// in the app rather than as a synonym.
    pub fn change_noun(self) -> &'static str {
        match self {
            Forge::Github => "pull request",
            Forge::Gitlab => "merge request",
        }
    }

    /// The sigil the forge itself writes before a change request's number.
    /// GitLab uses `!` and reserves `#` for issues, so a `#` on a GitLab row
    /// would name a different object entirely.
    pub fn number_sigil(self) -> &'static str {
        match self {
            Forge::Github => "#",
            Forge::Gitlab => "!",
        }
    }
}

/// What the user has said an instance is.
///
/// Deliberately **not** a variant on [`Forge`]. `Forge` answers "which adapter
/// drives this", and every arm of it must have one — a `Forge::GitOnly` would
/// have to be matched, and faked, in all seven dispatch functions at the bottom
/// of this file. `GitOnly` is the absence of an adapter, which is a different
/// statement and belongs in a different type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeclaredKind {
    Github,
    Gitlab,
    /// Git and nothing else: no pull requests, no merge requests, no API to ask.
    /// Declaring this is how a Gitea or a plain ssh remote stops being probed.
    GitOnly,
}

impl DeclaredKind {
    /// The adapter this declaration selects, or `None` for a host that has none.
    pub fn forge(self) -> Option<Forge> {
        match self {
            DeclaredKind::Github => Some(Forge::Github),
            DeclaredKind::Gitlab => Some(Forge::Gitlab),
            DeclaredKind::GitOnly => None,
        }
    }

    /// The value stored in `forge_instances.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            DeclaredKind::Github => "github",
            DeclaredKind::Gitlab => "gitlab",
            DeclaredKind::GitOnly => "git_only",
        }
    }

    /// Reads the column back. `None` for a value nothing recognises — unlike
    /// `Forge::from_column`, which can safely assume GitHub for a legacy row,
    /// there is no safe default here: guessing a forge for a host the user
    /// declared as something else would point Hangar at the wrong API.
    pub fn from_column(value: &str) -> Option<DeclaredKind> {
        match value {
            "github" => Some(DeclaredKind::Github),
            "gitlab" => Some(DeclaredKind::Gitlab),
            "git_only" => Some(DeclaredKind::GitOnly),
            _ => None,
        }
    }
}

/// The instances the user has declared, by host.
///
/// A process-wide registry rather than a lookup through `DbConnection`, because
/// [`Forge::for_host`] is called from `detect_repo` on paths that have no handle
/// on the database — the poller's, among them. Loaded once at startup
/// (`load_declared_hosts`) and rewritten whenever the user saves or deletes one.
static DECLARED_HOSTS: std::sync::Mutex<Option<std::collections::BTreeMap<String, DeclaredKind>>> = std::sync::Mutex::new(None);

/// What the user declared this host to be, if anything.
pub fn declared_kind(host: &str) -> Option<DeclaredKind> {
    let guard = DECLARED_HOSTS.lock().ok()?;
    guard.as_ref()?.get(&host.to_ascii_lowercase()).copied()
}

/// Replaces the registry wholesale.
///
/// Whole-map rather than per-entry so there is never a window where the registry
/// disagrees with the table it came from: a delete followed by a save would
/// otherwise leave the removed host live until the next write.
pub fn set_declared_hosts(hosts: std::collections::BTreeMap<String, DeclaredKind>) {
    if let Ok(mut guard) = DECLARED_HOSTS.lock() {
        *guard = Some(hosts);
    }
    // A host's kind changing invalidates every memo taken under the old
    // assumption — including "this host has no glab session", which is exactly
    // what a fresh declaration is meant to override.
    gitlab::forget_authenticated_hosts();
}

/// Whether the registry has been loaded at all. `false` means startup hasn't run
/// yet, which is different from "loaded and empty" — and only the first is worth
/// warning about.
pub fn declarations_loaded() -> bool {
    DECLARED_HOSTS.lock().is_ok_and(|g| g.is_some())
}

/// Serializes the tests that write the registry.
///
/// It is process-wide state and `cargo test` runs in threads, so two tests each
/// setting it up and tearing it down will read each other's teardown — which
/// they did, as a failure that only appeared once a second such test existed.
#[cfg(test)]
pub(crate) static DECLARATION_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Takes that lock, ignoring poisoning: a panicking test has already failed and
/// should not cascade into every other one.
#[cfg(test)]
pub(crate) fn lock_declarations_for_test() -> std::sync::MutexGuard<'static, ()> {
    DECLARATION_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Which forge, which instance, which project — everything needed to address
/// one repository.
///
/// Replaces the bare `(owner, repo)` tuple that used to be threaded through
/// `commands::pull_requests`, which had no way to say that `github.com/foo/bar`
/// and `gitlab.acme.com/foo/bar` are different repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub forge: Forge,
    /// Bare hostname, no scheme and no trailing slash: `github.com`,
    /// `gitlab.acme.com`.
    ///
    /// **Carries a port when, and only when, the remote was `http`/`https`** —
    /// `10.0.0.5:3000`. There the port is the endpoint, so two ports on one
    /// machine are two instances; over ssh it is only how git got there, and the
    /// same server answers its API on 443 either way. See [`split_remote_url`].
    pub host: String,
    /// Everything before the project slug: `killua15` on GitHub, or
    /// `group/subgroup` for a nested GitLab project. Persisted as `repo_owner`.
    pub namespace: String,
    /// The project slug. Persisted as `repo_name`.
    pub name: String,
}

impl RepoRef {
    /// `killua15/Hangar`, `group/subgroup/project` — what `--repo` takes on
    /// both CLIs, and what every screen displays.
    ///
    /// The only place the namespace and the name are rejoined, which is what
    /// lets a nested GitLab group live in `repo_owner` without a column of its
    /// own: nothing downstream ever reads either half alone.
    pub fn path(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

/// One comment, review or note, normalized.
///
/// `external_id` is the forge's own id — the actual "have we already seen this
/// one" key. Not a timestamp: a change request's history can grow a source of
/// comments Hangar couldn't previously fetch (inline diff comments were
/// invisible before they were added) — if "already surfaced" were tracked as
/// "newer than the latest timestamp seen so far", any such backfilled source
/// would stay permanently invisible the moment a single newer comment from any
/// other source had already advanced that watermark. Identity-based dedup has
/// no such blind spot: a comment is new exactly when Hangar has never persisted
/// its id before.
#[derive(Debug, Clone, PartialEq)]
pub struct RawPrComment {
    pub external_id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub path: Option<String>,
    pub line: Option<i64>,
}

/// Either the comments already in hand or what it takes to go and get them.
///
/// GitHub's one query carries every PR's comments for free; GitLab needs a
/// call per merge request. Lazy because [`decide_check_outcome`] answers
/// "nothing changed" on the overwhelming majority of passes, and on GitLab that
/// is the difference between one `glab` spawn per repo per pass and one per
/// open merge request.
///
/// Deliberately not an empty `Vec` for the unfetched case: "no comments" and
/// "not fetched yet" are different facts, and conflating them is how a merge
/// request's whole review history goes silently missing.
///
/// [`decide_check_outcome`]: crate::commands::pull_requests
#[derive(Debug, Clone, PartialEq)]
pub enum PendingComments {
    Ready(Vec<RawPrComment>),
    Deferred { repo: RepoRef, number: i64 },
}

impl PendingComments {
    pub fn resolve(self) -> Result<Vec<RawPrComment>, String> {
        match self {
            PendingComments::Ready(comments) => Ok(comments),
            PendingComments::Deferred { repo, number } => fetch_comments(&repo, number),
        }
    }
}

/// Everything one poll pass needs about one repo: the open change requests,
/// who the viewer is, and which of them are waiting on the viewer's review.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoPrSnapshot {
    /// The logged-in user, or `None` when the forge didn't say — see
    /// `classify_pr` for why that isn't treated as "everything is mine".
    pub viewer_login: Option<String>,
    pub review_requested: HashSet<i64>,
    pub prs: Vec<SnapshotPr>,
}

/// One open change request as the forge returned it.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotPr {
    pub number: i64,
    pub url: String,
    pub title: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// `open` / `closed` / `merged`, lowercased to match the `state` column.
    pub state: String,
    pub author_login: Option<String>,
    pub assignees: Vec<String>,
    pub head_branch: Option<String>,
    pub base_branch: Option<String>,
    pub comments: PendingComments,
}

/// What `check_pr_comments_with` needs to judge one change request, from either
/// source: a repo-wide snapshot (the poller) or a single-PR query (the
/// per-session "check now"). Splitting this out is what lets one repo query
/// serve every tracked change request in it instead of one query each.
#[derive(Debug, Clone, PartialEq)]
pub struct PrFacts {
    pub state: String,
    pub updated_at: String,
    pub comments: PendingComments,
    pub viewer_login: Option<String>,
}

/// One repository on an instance, as the "browse and clone" screen needs it.
///
/// Same normalization job [`SnapshotPr`] does for change requests: GitHub's
/// `full_name`/`clone_url` and GitLab's `path_with_namespace`/`ssh_url_to_repo`
/// are the same four facts under different names, and the screen should not have
/// to know which forge it is looking at.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceRepo {
    /// `group/subgroup/project` — what identifies it, and what the folder is
    /// named after.
    pub path: String,
    /// The short name, for a list that would otherwise repeat the namespace on
    /// every row.
    pub name: String,
    pub description: String,
    pub web_url: String,
    /// What `git clone` is handed. SSH where the forge offers it: on a
    /// self-hosted instance that is nearly always what is actually configured,
    /// and it clones without a token.
    pub clone_url: String,
}

/// The repositories this instance's login can see.
pub fn list_repos(host: &str) -> Result<Vec<InstanceRepo>, String> {
    match Forge::for_host(host) {
        Some(Forge::Github) => github::list_repos(host),
        Some(Forge::Gitlab) => gitlab::list_repos(host),
        // Not a refusal to be helpful: a git-only host has no API to ask, and a
        // host nothing has placed cannot be asked in any particular dialect.
        None => Err(format!("{host} has no API Hangar can browse — declare it as GitHub or GitLab first")),
    }
}

/// Signs the instance's CLI in with a token, passed on stdin.
pub fn login_with_token(host: &str, token: &str) -> Result<(), String> {
    match Forge::for_host(host) {
        Some(Forge::Github) => github::login_with_token(host, token),
        Some(Forge::Gitlab) => gitlab::login_with_token(host, token),
        None => Err(format!("{host} isn't declared as a forge, so there is no CLI to sign in")),
    }
}

/// What to open, for [`create_change_request`].
pub struct NewChangeRequest<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub base: &'a str,
    pub head: &'a str,
}

/// What came back. The number is the forge's own — GitHub's repo-wide
/// `number`, GitLab's project-scoped `iid` — and both are what their URLs and
/// their CLIs address.
pub struct CreatedChangeRequest {
    pub url: String,
    pub number: i64,
}

/// Splits a git remote URL into its host and its project path.
///
/// Handles the three forms a remote actually takes: `https://host/path`,
/// `git@host:path` and `ssh://git@host:2222/path`. Deliberately string
/// surgery rather than a URL crate — the scp-like `git@host:path` form is not
/// a URL at all, so a parser would have to be special-cased for it anyway.
///
/// **The port survives only on `http`/`https`**, and the asymmetry is the whole
/// point. An SSH port says nothing about where the instance's API lives: a
/// GitLab cloned over `ssh://git@gitlab.acme.com:2222/…` still answers its API
/// on `https://gitlab.acme.com`, so carrying `:2222` into the host would invent
/// a second instance for the same server. An HTTP port is the opposite — it
/// *is* the endpoint. A forge on `http://10.0.0.5:3000` has its API at
/// `http://10.0.0.5:3000/api/…` and nowhere else, so dropping the port leaves a
/// host that addresses a different machine's port 443.
fn split_remote_url(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    // Read the scheme before stripping it: it decides whether a port is part of
    // the instance's identity or just how git got there.
    let (scheme, after_scheme) = match trimmed.split_once("://") {
        Some((scheme, rest)) => (Some(scheme.to_ascii_lowercase()), rest),
        None => (None, trimmed),
    };
    let port_is_identity = matches!(scheme.as_deref(), Some("http") | Some("https"));
    let after_user = after_scheme.rsplit_once('@').map(|(_, rest)| rest).unwrap_or(after_scheme);

    // `:` separates host from path in the scp form and host from port in the
    // ssh one — the difference is whether what follows is a number.
    let (host_part, path) = match after_user.find(['/', ':']) {
        Some(idx) => (&after_user[..idx], &after_user[idx + 1..]),
        None => return None,
    };
    // Only a `:` can have introduced a port. Without this a numeric first path
    // segment reads as one — `https://host/1234/proj` would lose `1234` and then
    // fail the "must contain a slash" check below, rejecting a perfectly good
    // remote.
    let separated_by_colon = after_user.as_bytes().get(host_part.len()) == Some(&b':');
    let (host, path) = match path.split_once('/') {
        Some((maybe_port, rest)) if separated_by_colon && !maybe_port.is_empty() && maybe_port.chars().all(|c| c.is_ascii_digit()) => {
            let host = if port_is_identity { format!("{host_part}:{maybe_port}") } else { host_part.to_string() };
            (host, rest)
        }
        _ => (host_part.to_string(), path),
    };

    let path = path.trim_start_matches('/');
    (!host.is_empty() && path.contains('/')).then(|| (host.to_ascii_lowercase(), path.to_string()))
}

/// Just the instance a remote URL names, with no opinion about whether Hangar
/// can speak to it.
///
/// [`parse_repo_from_url`] answers `None` for a host it can't place, which is
/// right for "which repo is this" and useless for "which instances do this
/// user's projects live on" — the unplaceable ones are precisely the ones worth
/// reporting. Same parse either way, so the scp/ssh handling stays in one
/// place.
pub fn parse_host_from_url(url: &str) -> Option<String> {
    split_remote_url(url).map(|(host, _)| host)
}

/// The repository a remote URL points at, or `None` when it points at
/// something Hangar has no adapter for.
pub fn parse_repo_from_url(url: &str) -> Option<RepoRef> {
    let (host, path) = split_remote_url(url)?;
    let forge = Forge::for_host(&host)?;
    // `rsplit_once`, not `splitn(2, …)`: the project is the *last* segment, and
    // everything before it is the namespace. That is what lets a nested GitLab
    // group (`group/subgroup/project`) live in `repo_owner` without a column of
    // its own, and it is identical to a first-segment split whenever there are
    // only two segments — which is every GitHub repository.
    let (namespace, name) = path.rsplit_once('/')?;
    (!namespace.is_empty() && !name.is_empty()).then(|| RepoRef { forge, host, namespace: namespace.to_string(), name: name.to_string() })
}

/// The repository `cwd`'s `origin` points at.
pub fn detect_repo(cwd: &Path) -> Result<RepoRef, String> {
    let url = crate::commands::pull_requests::git_capture(cwd, &["remote", "get-url", "origin"])?;
    parse_repo_from_url(&url).ok_or_else(|| format!("could not parse an owner/repo out of remote url: {url}"))
}

/// Clear, actionable error instead of letting a bare create/list failure
/// (missing binary or expired login) surface as a cryptic exit-code message.
pub fn available_and_authed(repo: &RepoRef) -> Result<(), String> {
    match repo.forge {
        Forge::Github => github::available_and_authed(),
        Forge::Gitlab => gitlab::available_and_authed(),
    }
}

/// One round trip for one repo. `limit` caps the open change requests pulled
/// in — see `DISCOVERY_PR_LIMIT`.
pub fn fetch_repo_snapshot(repo: &RepoRef, limit: usize) -> Result<RepoPrSnapshot, String> {
    match repo.forge {
        Forge::Github => github::fetch_repo_snapshot(repo, limit),
        Forge::Gitlab => gitlab::fetch_repo_snapshot(repo, limit),
    }
}

/// One change request, for the paths that don't have a repo snapshot to hand:
/// the per-session "check now", and the poller resolving a tracked one that has
/// dropped off the open list (merged or closed — the repo query only returns
/// open ones, so absence is the signal but not the reason).
pub fn fetch_single_pr_facts(repo: &RepoRef, number: i64) -> Result<PrFacts, String> {
    match repo.forge {
        Forge::Github => github::fetch_single_pr_facts(repo, number),
        Forge::Gitlab => gitlab::fetch_single_pr_facts(repo, number),
    }
}

/// One change request's comments, for the forges that don't carry them in the
/// snapshot. Reached through [`PendingComments::resolve`], never called
/// directly by the orchestration.
pub fn fetch_comments(repo: &RepoRef, number: i64) -> Result<Vec<RawPrComment>, String> {
    match repo.forge {
        Forge::Github => github::fetch_comments(repo, number),
        Forge::Gitlab => gitlab::fetch_comments(repo, number),
    }
}

pub fn create_change_request(repo: &RepoRef, cwd: &Path, req: &NewChangeRequest) -> Result<CreatedChangeRequest, String> {
    match repo.forge {
        Forge::Github => github::create_change_request(repo, cwd, req),
        Forge::Gitlab => gitlab::create_change_request(repo, req),
    }
}

/// The branch a change request was opened from — backfilled lazily for one
/// Hangar didn't open itself.
pub fn fetch_head_branch(repo: &RepoRef, number: i64) -> Result<String, String> {
    match repo.forge {
        Forge::Github => github::fetch_head_branch(repo, number),
        Forge::Gitlab => gitlab::fetch_head_branch(repo, number),
    }
}

/// The repo's default branch according to the forge, for when git itself
/// doesn't know locally. `None` rather than an error: every caller has a
/// reasonable fallback anyway.
pub fn default_branch(repo: &RepoRef, cwd: &Path) -> Option<String> {
    match repo.forge {
        Forge::Github => github::default_branch(cwd),
        Forge::Gitlab => gitlab::default_branch(repo),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github(namespace: &str, name: &str) -> Option<RepoRef> {
        Some(RepoRef { forge: Forge::Github, host: "github.com".into(), namespace: namespace.into(), name: name.into() })
    }

    #[test]
    fn parse_repo_from_url_handles_https_and_ssh_forms() {
        assert_eq!(parse_repo_from_url("https://github.com/killua15/Hangar.git"), github("killua15", "Hangar"));
        assert_eq!(parse_repo_from_url("https://github.com/killua15/Hangar"), github("killua15", "Hangar"));
        assert_eq!(parse_repo_from_url("git@github.com:killua15/Hangar.git"), github("killua15", "Hangar"));
    }

    #[test]
    fn parse_repo_from_url_rejects_what_it_cannot_place() {
        assert_eq!(parse_repo_from_url("not a url"), None);
        assert_eq!(parse_repo_from_url("https://github.com/killua15"), None, "a host with no project path is not a repo");
        assert_eq!(parse_repo_from_url(""), None);
    }

    /// **The subgroup contract.** GitLab projects nest arbitrarily deep, and
    /// the whole namespace has to survive into `repo_owner` — a first-segment
    /// split would silently turn `group/subgroup/project` into the project
    /// `subgroup/project` under the group `group`, and every `--repo` and API
    /// path built from it would then address something that doesn't exist.
    #[test]
    fn parse_repo_from_url_keeps_nested_subgroups_in_the_namespace() {
        let repo = parse_repo_from_url("https://gitlab.com/group/subgroup/project.git").unwrap();
        assert_eq!(repo.forge, Forge::Gitlab);
        assert_eq!(repo.namespace, "group/subgroup");
        assert_eq!(repo.name, "project");
        assert_eq!(repo.path(), "group/subgroup/project", "and it rejoins to exactly what it came from");
    }

    #[test]
    fn parse_repo_from_url_reads_the_host_not_the_scheme() {
        // A self-hosted instance, over ssh, on a non-default port, nested.
        let repo = parse_repo_from_url("ssh://git@gitlab.acme.com:2222/group/sub/proj.git").unwrap();
        assert_eq!(repo.host, "gitlab.acme.com", "the port belongs to the transport, not the identity of the instance");
        assert_eq!(repo.forge, Forge::Gitlab);
        assert_eq!(repo.path(), "group/sub/proj");

        let scp = parse_repo_from_url("git@gitlab.acme.com:group/proj.git").unwrap();
        assert_eq!(scp.host, "gitlab.acme.com");
        assert_eq!(scp.path(), "group/proj");
    }

    /// A self-hosted GitLab almost always says so in its own hostname, and
    /// that is the signal — no login, no spawn, no guess.
    #[test]
    fn for_host_recognises_gitlab_by_name() {
        assert_eq!(Forge::for_host("github.com"), Some(Forge::Github));
        assert_eq!(Forge::for_host("GitHub.com"), Some(Forge::Github), "hostnames are case-insensitive");
        assert_eq!(Forge::for_host("gitlab.com"), Some(Forge::Gitlab));
        assert_eq!(Forge::for_host("gitlab.acme.com"), Some(Forge::Gitlab));
        assert_eq!(Forge::for_host("code.gitlab.acme.internal"), Some(Forge::Gitlab));
    }

    /// The host comes back for **any** remote, including one on a vanity domain
    /// no adapter claims — which is the only reason this exists next to
    /// `parse_repo_from_url`. An instance has to be *reportable* before it is
    /// usable, and `parse_repo_from_url` answers `None` for exactly the hosts
    /// worth reporting.
    ///
    /// Deliberately asserts nothing about which forge these hosts *are*: that
    /// depends on whether `glab` happens to have a session on the machine
    /// running the test, and a test whose result moves with the developer's
    /// login state is worse than no test.
    #[test]
    fn parse_host_from_url_answers_for_hosts_no_adapter_claims() {
        assert_eq!(parse_host_from_url("git@git.acme.com:tms-3.0/TMS-server.git").as_deref(), Some("git.acme.com"));
        assert_eq!(parse_host_from_url("https://github.com/killua15/Hangar.git").as_deref(), Some("github.com"));
        assert_eq!(parse_host_from_url("ssh://git@gitlab.acme.com:2222/group/sub/proj.git").as_deref(), Some("gitlab.acme.com"), "the port is transport, not identity");
        assert_eq!(parse_host_from_url("not a url"), None);
        assert_eq!(parse_host_from_url("https://github.com/killua15").as_deref(), None, "a host with no project path is not a remote");
    }

    /// **The port asymmetry, both halves.**
    ///
    /// An HTTP port is the endpoint: a forge on `http://10.0.0.5:3000` answers
    /// its API there and nowhere else, so two ports on one machine are two
    /// instances and dropping the port addresses a different server entirely. An
    /// SSH port is only how git got there — the same GitLab answers its API on
    /// 443 whether it was cloned over 22 or 2222 — so keeping it would invent a
    /// second instance for one server.
    #[test]
    fn a_port_belongs_to_the_instance_over_http_and_to_the_transport_over_ssh() {
        // The live case: a forge on a bare IP and a non-default port.
        let http = parse_host_from_url("http://52.188.59.199:3000/WPLANNING/wplanning-app-mobile.git");
        assert_eq!(http.as_deref(), Some("52.188.59.199:3000"), "an http port is part of the instance");
        assert_eq!(parse_host_from_url("https://gitlab.acme.com:8443/g/p").as_deref(), Some("gitlab.acme.com:8443"));

        // Same server, reached over ssh: no port in the identity.
        assert_eq!(parse_host_from_url("ssh://git@gitlab.acme.com:2222/g/p").as_deref(), Some("gitlab.acme.com"));
        assert_eq!(parse_host_from_url("git@gitlab.acme.com:g/p").as_deref(), Some("gitlab.acme.com"));

        // And two ports on one machine stay apart.
        assert_ne!(parse_host_from_url("http://10.0.0.5:3000/g/p"), parse_host_from_url("http://10.0.0.5:3001/g/p"));
    }

    /// The default port isn't written out, so an https remote with no port keeps
    /// a bare host — otherwise the same instance would have two identities
    /// depending on how thoroughly someone typed the URL.
    #[test]
    fn a_default_port_leaves_the_host_bare() {
        assert_eq!(parse_host_from_url("https://gitlab.com/group/proj.git").as_deref(), Some("gitlab.com"));
    }

    /// A numeric first path segment is not a port. Without the `:` check it read
    /// as one, and the remote was then rejected outright for having no slash
    /// left in its path.
    #[test]
    fn a_numeric_namespace_is_not_mistaken_for_a_port() {
        let repo = parse_repo_from_url("https://gitlab.com/1234/proj.git").expect("a numeric group is still a group");

        assert_eq!(repo.host, "gitlab.com", "no port was in that URL at all");
        assert_eq!(repo.path(), "1234/proj");
    }

    #[test]
    fn repo_ref_path_rejoins_namespace_and_name() {
        let repo = RepoRef { forge: Forge::Github, host: "github.com".into(), namespace: "killua15".into(), name: "Hangar".into() };
        assert_eq!(repo.path(), "killua15/Hangar");
    }

    /// Tests share one process, and the registry is process-wide — so anything
    /// touching it has to hold the lock and put it back, or it both leaks into
    /// and is trampled by whatever runs beside it.
    fn with_declarations<T>(hosts: &[(&str, DeclaredKind)], body: impl FnOnce() -> T) -> T {
        let _guard = lock_declarations_for_test();
        let before = DECLARED_HOSTS.lock().unwrap().clone();
        set_declared_hosts(hosts.iter().map(|(h, k)| (h.to_string(), *k)).collect());
        let out = body();
        *DECLARED_HOSTS.lock().unwrap() = before;
        out
    }

    /// **The precedence rule.** Everything `for_host` does without a declaration
    /// is a guess about someone else's server; the user's word settles it. The
    /// case that matters: a GitHub Enterprise on a vanity domain, which no
    /// heuristic can reach at all.
    #[test]
    fn a_declaration_beats_every_heuristic() {
        with_declarations(&[("git.acme.com", DeclaredKind::Github)], || {
            assert_eq!(Forge::for_host("git.acme.com"), Some(Forge::Github), "no heuristic finds a GHE on a vanity domain");
        });

        // Even against the hostname heuristic, which would have said GitLab.
        with_declarations(&[("gitlab.acme.com", DeclaredKind::Github)], || {
            assert_eq!(Forge::for_host("gitlab.acme.com"), Some(Forge::Github), "the name is a guess; the declaration is not");
        });
    }

    /// The point of `GitOnly`: `for_host` answers `None` **without** probing. Left
    /// undeclared, this host pays a `glab auth status` spawn on every poll pass,
    /// forever, to fail identically each time.
    #[test]
    fn a_git_only_declaration_stops_the_probing() {
        with_declarations(&[("52.188.59.199:3000", DeclaredKind::GitOnly)], || {
            assert_eq!(Forge::for_host("52.188.59.199:3000"), None);
        });
    }

    #[test]
    fn declarations_are_matched_case_insensitively_like_every_other_host_lookup() {
        with_declarations(&[("git.acme.com", DeclaredKind::Gitlab)], || {
            assert_eq!(Forge::for_host("Git.Acme.COM"), Some(Forge::Gitlab));
        });
    }

    #[test]
    fn declared_kind_round_trips_through_its_column() {
        for kind in [DeclaredKind::Github, DeclaredKind::Gitlab, DeclaredKind::GitOnly] {
            assert_eq!(DeclaredKind::from_column(kind.as_str()), Some(kind));
        }
        // No safe default: guessing a forge for a host declared as something
        // else would point Hangar at the wrong API.
        assert_eq!(DeclaredKind::from_column("bitbucket"), None);
        assert_eq!(DeclaredKind::from_column(""), None);
    }

    #[test]
    fn only_github_and_gitlab_declarations_select_an_adapter() {
        assert_eq!(DeclaredKind::Github.forge(), Some(Forge::Github));
        assert_eq!(DeclaredKind::Gitlab.forge(), Some(Forge::Gitlab));
        assert_eq!(DeclaredKind::GitOnly.forge(), None, "git-only means there is no adapter, not that there is a third one");
    }

    #[test]
    fn forge_from_column_treats_anything_unknown_as_github() {
        // Rows written before the column existed read back as `NULL`-free
        // `'github'`, and a value nobody recognises is likelier a downgrade
        // than a third forge — either way, guessing GitHub matches the
        // column's own DEFAULT.
        assert_eq!(Forge::from_column("github"), Forge::Github);
        assert_eq!(Forge::from_column("gitlab"), Forge::Gitlab);
        assert_eq!(Forge::from_column(""), Forge::Github);
        assert_eq!(Forge::from_column("bitbucket"), Forge::Github);
    }
}
