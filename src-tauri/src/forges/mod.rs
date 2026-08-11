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
    pub fn for_host(host: &str) -> Option<Forge> {
        let host = host.to_ascii_lowercase();
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

/// Which forge, which instance, which project — everything needed to address
/// one repository.
///
/// Replaces the bare `(owner, repo)` tuple that used to be threaded through
/// `commands::pull_requests`, which had no way to say that `github.com/foo/bar`
/// and `gitlab.acme.com/foo/bar` are different repositories.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub forge: Forge,
    /// Bare hostname — no scheme, no port, no trailing slash: `github.com`,
    /// `gitlab.acme.com`.
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
fn split_remote_url(url: &str) -> Option<(String, String)> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    // Strip the scheme, then any `user@` in front of the host.
    let after_scheme = trimmed.split_once("://").map(|(_, rest)| rest).unwrap_or(trimmed);
    let after_user = after_scheme.rsplit_once('@').map(|(_, rest)| rest).unwrap_or(after_scheme);

    // `:` separates host from path in the scp form and host from port in the
    // ssh one — the difference is whether what follows is a number.
    let (host_part, path) = match after_user.find(['/', ':']) {
        Some(idx) => (&after_user[..idx], &after_user[idx + 1..]),
        None => return None,
    };
    let (host, path) = match path.split_once('/') {
        Some((maybe_port, rest)) if !maybe_port.is_empty() && maybe_port.chars().all(|c| c.is_ascii_digit()) => (host_part, rest),
        _ => (host_part, path),
    };

    let path = path.trim_start_matches('/');
    (!host.is_empty() && path.contains('/')).then(|| (host.to_ascii_lowercase(), path.to_string()))
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

    #[test]
    fn repo_ref_path_rejoins_namespace_and_name() {
        let repo = RepoRef { forge: Forge::Github, host: "github.com".into(), namespace: "killua15".into(), name: "Hangar".into() };
        assert_eq!(repo.path(), "killua15/Hangar");
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
