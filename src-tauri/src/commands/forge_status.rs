//! Which forge instances this user's projects actually live on, and whether
//! Hangar can talk to them.
//!
//! The gap this fills was a silent one. `poll_all_open_prs` skips any project
//! whose remote it can't place — "no git remote, or one on a host no adapter
//! speaks — nothing to discover either way, and neither is an error" — and it
//! is right that that isn't an error. But it left the user with two GitLab
//! projects that produced no merge requests and no way at all to find out why:
//! `pull_requests.last_check_error` only exists once a change request is being
//! tracked, and these never got that far.
//!
//! A host on a vanity domain (`git.acme.com`) is the case that makes this
//! necessary rather than nice. [`Forge::for_host`] can only recognise it by
//! asking whether `glab` has a session there, so before the first login it is
//! indistinguishable from a host Hangar will never support — and the fix and
//! the diagnosis are the same sentence: log in, and it becomes a GitLab.
//!
//! Deliberately **not** part of [`list_requirements`]. That renders a screen
//! and uses `check_offline` so opening Settings never waits on the network;
//! this asks each instance whether it is logged in, which spawns a process and
//! makes a real API call. Two questions, two commands, and the expensive one is
//! the one the user asks for.
//!
//! [`Forge::for_host`]: crate::forges::Forge::for_host
//! [`list_requirements`]: crate::commands::requirements::list_requirements

use crate::db::DbConnection;
use crate::forges::{self, DeclaredKind, Forge};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;
use tauri::{AppHandle, Manager, State};

/// One project resting on an instance. Named so the user can see *what stops
/// working* rather than being told a hostname is unreachable in the abstract.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ForgeInstanceProject {
    pub id: String,
    pub name: String,
}

/// One instance, named and counted and nothing more — what a picker needs.
///
/// Deliberately not `ForgeInstanceStatus`: everything that struct adds about
/// logins costs a network probe, and this is answered from the database and a
/// local `git` call.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ForgeHost {
    pub host: String,
    /// `None` for a host nothing places — still a real choice, since projects
    /// point at it.
    pub forge: Option<String>,
    /// What the user called it when declaring it. Empty otherwise.
    pub label: String,
    pub project_count: usize,
}

/// One instance, and whether it is usable.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ForgeInstanceStatus {
    /// Bare hostname, as `RepoRef::host` stores it: `github.com`,
    /// `gitlab.acme.com`.
    pub host: String,
    /// `None` when nothing can place this host yet — which is a state worth
    /// rendering, not one to hide.
    pub forge: Option<String>,
    pub authenticated: bool,
    /// One sentence, addressed to the user, when this instance isn't usable.
    /// Shaped after `capabilities::Blocked::reason` so both read alike.
    pub reason: Option<String>,
    /// The command that fixes it. Always present when `authenticated` is false:
    /// a row that says something is broken and not what to do about it is the
    /// dead end this module exists to remove.
    pub login_command: Option<String>,
    pub projects: Vec<ForgeInstanceProject>,
    /// Whether the user said what this is, or Hangar guessed. A guess can be
    /// wrong and the screen has to offer to correct it; a declaration should not
    /// invite second-guessing.
    pub declared: bool,
    /// What the user calls it. Empty unless declared — nothing else has a name
    /// to offer.
    pub label: String,
    /// Git works here and there is no API to ask. Distinct from "unplaceable":
    /// this one is settled, and settled is why the probing stops.
    pub git_only: bool,
    /// Who you are on this instance, when anything can say — the API when it is
    /// reachable, the SSH greeting when it isn't.
    pub viewer_login: Option<String>,
}

/// Reads the declarations out of the database and into the process-wide
/// registry.
///
/// Called from `lib.rs`'s `.setup()` **before the poller thread starts**, and the
/// order is the point: a declaration loaded afterwards would let the first poll
/// pass run on the heuristics the user already overrode — probing a git-only host
/// and mis-placing a vanity domain exactly once per launch, which is the kind of
/// bug that only ever reproduces on the first pass.
pub fn load_declared_hosts(db: &DbConnection) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT host, kind FROM forge_instances").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    // A row whose `kind` nothing recognises is skipped rather than guessed at:
    // the column has a CHECK constraint, so one can only appear on a database
    // written by a newer build, and picking a forge for it would point Hangar at
    // the wrong API.
    let declared = rows.into_iter().filter_map(|(host, kind)| DeclaredKind::from_column(&kind).map(|k| (host, k))).collect();
    forges::set_declared_hosts(declared);
    Ok(())
}

/// Every instance the registered projects point at, with its login state.
#[tauri::command]
pub fn list_forge_instances(db: State<DbConnection>) -> Result<Vec<ForgeInstanceStatus>, String> {
    list_forge_instances_impl(&db)
}

/// Just the instances and how many projects sit on each — no login probing.
///
/// `list_forge_instances` above answers "is this usable and how do I fix it",
/// and pays for it: `place()` runs `gh auth status`, `glab auth status`, and an
/// SSH round trip per host. A picker only needs the names, and asking it that
/// question on a screen that hasn't loaded anything yet would put a network
/// wait exactly where we just removed one. `group_by_host` was already split
/// out for being the half that touches nothing.
#[tauri::command]
pub async fn list_forge_hosts<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<Vec<ForgeHost>, String> {
    // Still `run_blocking`: reading each project's `origin` is a `git` call per
    // project, which is local and quick but not free.
    crate::blocking::run_blocking(move || {
        let db = app.state::<DbConnection>();
        let declarations = read_declarations(&db)?;
        let projects: Vec<(String, String, String)> = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let mut stmt = conn.prepare("SELECT id, name, path FROM projects ORDER BY name").map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            rows
        };

        let mut grouped = group_by_host(projects);
        // A declared instance with no project on it yet still belongs in the
        // picker — same reason `list_forge_instances` includes it.
        for host in declarations.keys() {
            if !grouped.iter().any(|(known, _)| known == host) {
                grouped.push((host.clone(), Vec::new()));
            }
        }

        Ok(grouped
            .into_iter()
            .map(|(host, projects)| {
                // The declaration when there is one, the hostname's own answer
                // otherwise — both free. An instance nothing can place still
                // gets listed: it has projects on it, so it is a real choice.
                let forge = declarations
                    .get(&host)
                    .and_then(|d| d.kind.forge())
                    .or_else(|| forges::Forge::for_host(&host))
                    .map(|f| f.as_str().to_string());
                let label = declarations.get(&host).map(|d| d.label.clone()).unwrap_or_default();
                ForgeHost { host, forge, label, project_count: projects.len() }
            })
            .collect())
    })
    .await?
}

/// The same, after throwing away what was memoized about who is logged in
/// where.
///
/// The counterpart to `recheck_requirements`: the user has just been handed a
/// terminal to log in with, so the one thing that must not happen is the app
/// answering from a cache taken before they did.
#[tauri::command]
pub fn recheck_forge_instances(db: State<DbConnection>) -> Result<Vec<ForgeInstanceStatus>, String> {
    forges::gitlab::forget_authenticated_hosts();
    list_forge_instances_impl(&db)
}

/// Every declared instance, by host.
fn read_declarations(db: &DbConnection) -> Result<BTreeMap<String, Declaration>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn.prepare("SELECT host, kind, scheme, label FROM forge_instances").map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .filter_map(|(host, kind, scheme, label)| DeclaredKind::from_column(&kind).map(|kind| (host, Declaration { kind, scheme, label })))
        .collect())
}

fn list_forge_instances_impl(db: &DbConnection) -> Result<Vec<ForgeInstanceStatus>, String> {
    let declarations = read_declarations(db)?;
    let projects: Vec<(String, String, String)> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn.prepare("SELECT id, name, path FROM projects ORDER BY name").map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        rows
    };

    // Declared instances appear even with no project on them yet — that is the
    // whole point of declaring one before cloning anything from it.
    let mut grouped = group_by_host(projects);
    for host in declarations.keys() {
        if !grouped.iter().any(|(known, _)| known == host) {
            grouped.push((host.clone(), Vec::new()));
        }
    }

    Ok(grouped.into_iter().map(|(host, projects)| status_for(host, projects, &declarations)).collect())
}

/// Projects to instances, by reading each one's `origin`.
///
/// Grouped **before** any login is probed, so an instance with four projects on
/// it costs one `glab auth status` and one SSH round trip rather than four of
/// each. Separate from the probing so it can be tested against real checkouts
/// without waiting on a network.
fn group_by_host(projects: Vec<(String, String, String)>) -> Vec<(String, Vec<ForgeInstanceProject>)> {
    let mut hosts: Vec<(String, Vec<ForgeInstanceProject>)> = Vec::new();
    for (id, name, path) in projects {
        let Some(host) = host_of_project(Path::new(&path)) else {
            // No remote at all, or not a repo. Nothing to report: this project
            // isn't waiting on any instance.
            continue;
        };
        let project = ForgeInstanceProject { id, name };
        match hosts.iter_mut().find(|(known, _)| *known == host) {
            Some((_, on_host)) => on_host.push(project),
            None => hosts.push((host, vec![project])),
        }
    }
    hosts
}

fn host_of_project(root: &Path) -> Option<String> {
    let url = crate::commands::pull_requests::git_capture(root, &["remote", "get-url", "origin"]).ok()?;
    forges::parse_host_from_url(&url)
}

/// What could be learned about one instance, and by which route.
///
/// Split from the message so every sentence the user reads is testable without
/// a network — the probes below are the only part that touches one.
#[derive(Debug, Clone, PartialEq)]
enum Placement {
    Github { authed: bool, viewer: Option<String> },
    Gitlab { authed: bool, ssh_user: Option<String>, viewer: Option<String> },
    /// Declared as git and nothing more. A settled answer, not a failure — and
    /// the only one that costs no probing at all.
    GitOnly,
    /// Nothing places it: not its name, not a `glab` session, not even an SSH
    /// greeting.
    Unknown,
}

/// The probing half. Every call here can spawn or hit the network, which is why
/// this command is separate from `list_requirements`.
fn place(host: &str) -> Placement {
    // Declared git-only short-circuits everything, and that is the point of the
    // declaration: no `glab auth status`, no SSH round trip, no guessing — on
    // this pass or any other.
    if forges::declared_kind(host) == Some(DeclaredKind::GitOnly) {
        return Placement::GitOnly;
    }
    match Forge::for_host(host) {
        Some(Forge::Github) => {
            let authed = gh_is_authenticated();
            Placement::Github { authed, viewer: authed.then(|| forges::github::viewer_login(host)).flatten() }
        }
        Some(Forge::Gitlab) => {
            let authed = forges::gitlab::is_authenticated(host);
            // Only worth the ~2s round trip when the API is the thing that's
            // missing: an authenticated instance has nothing left to explain.
            let ssh_user = (!authed).then(|| forges::gitlab::ssh_identity(host)).flatten();
            let viewer = authed.then(|| forges::gitlab::viewer_login(host)).flatten().or_else(|| ssh_user.clone());
            Placement::Gitlab { authed, ssh_user, viewer }
        }
        // A GitLab on a vanity domain reaches here: `for_host` can only
        // recognise it by a `glab` session, and there isn't one yet. The SSH
        // greeting settles it without any token — `Welcome to GitLab, @user!`
        // is the instance identifying both itself and you.
        None => match forges::gitlab::ssh_identity(host) {
            Some(user) => Placement::Gitlab { authed: false, ssh_user: Some(user.clone()), viewer: Some(user) },
            None => Placement::Unknown,
        },
    }
}

/// The message half — pure, so the wording is under test.
fn status_from(host: String, projects: Vec<ForgeInstanceProject>, placement: Placement, declaration: Option<Declaration>) -> ForgeInstanceStatus {
    let glab_login = format!("glab auth login --hostname {host}");
    let (declared, label) = match &declaration {
        Some(d) => (true, d.label.clone()),
        None => (false, String::new()),
    };
    let base = ForgeInstanceStatus {
        host: host.clone(),
        forge: None,
        authenticated: false,
        reason: None,
        login_command: None,
        projects,
        declared,
        label,
        git_only: false,
        viewer_login: None,
    };
    match placement {
        Placement::Github { authed, viewer } => ForgeInstanceStatus {
            forge: Some(Forge::Github.as_str().to_string()),
            authenticated: authed,
            reason: (!authed).then(|| "the GitHub CLI isn't logged in".to_string()),
            login_command: (!authed).then(|| format!("gh auth login --hostname {host}")),
            viewer_login: viewer,
            ..base
        },
        Placement::Gitlab { authed, ssh_user, viewer } => ForgeInstanceStatus {
            forge: Some(Forge::Gitlab.as_str().to_string()),
            authenticated: authed,
            // Naming the SSH identity is the difference between "this is
            // broken" and "this is *half* set up". Someone whose key clones and
            // pushes all day reasonably assumes GitLab is configured; the thing
            // they're missing is an API token, and only saying so explains why
            // git works and merge requests don't.
            reason: (!authed).then(|| match &ssh_user {
                Some(user) => format!(
                    "your SSH key works on {host} — it greets you as @{user}, which is how Hangar knows it's a GitLab. Listing merge requests goes through the REST API, and that takes a token, not an SSH key"
                ),
                None => format!("`glab` has no session on {host}"),
            }),
            login_command: (!authed).then_some(glab_login),
            viewer_login: viewer,
            ..base
        },
        // Not an error state and not a broken one: the user said this host is
        // git and nothing else, so there is nothing to authenticate and nothing
        // to fix. `authenticated: true` would be a lie, which is why the screen
        // reads `git_only` instead of inferring from it.
        Placement::GitOnly => ForgeInstanceStatus { forge: None, authenticated: false, git_only: true, ..base },
        Placement::Unknown => ForgeInstanceStatus {
            forge: None,
            authenticated: false,
            reason: Some(format!(
                "Hangar can't tell what {host} is. It isn't github.com, its name doesn't say GitLab, and it didn't answer an SSH greeting — declare what it is, or sign in with `glab` if it's a GitLab"
            )),
            login_command: Some(glab_login),
            ..base
        },
    }
}

fn status_for(host: String, projects: Vec<ForgeInstanceProject>, declarations: &BTreeMap<String, Declaration>) -> ForgeInstanceStatus {
    let placement = place(&host);
    let declaration = declarations.get(&host).cloned();
    status_from(host, projects, placement, declaration)
}

/// A row of `forge_instances`, for the parts of it the registry doesn't hold.
/// `DECLARED_HOSTS` carries only the kind, because that is all `for_host` needs;
/// the label and scheme are for the screen.
#[derive(Debug, Clone, PartialEq)]
struct Declaration {
    kind: DeclaredKind,
    scheme: String,
    label: String,
}

/// Same question `capabilities::check` asks for `NeedsLogin::Gh`, which keeps
/// its own copy private.
fn gh_is_authenticated() -> bool {
    forges::github::gh_output(None, &["auth", "status"]).is_ok_and(|o| o.status.success())
}

// -- declaring, authenticating, browsing ------------------------------------

/// Records what the user says a host is.
///
/// Upsert on `host`, because declaring the same instance twice is a correction,
/// not a second instance — and the primary key would reject the second write
/// anyway.
#[tauri::command]
pub fn save_forge_instance(
    db: State<DbConnection>,
    host: String,
    kind: String,
    scheme: Option<String>,
    label: Option<String>,
) -> Result<Vec<ForgeInstanceStatus>, String> {
    // Lowercased on the way in: every host lookup in the app is
    // case-insensitive, and a row keyed `GitLab.Acme.com` would simply never be
    // found again.
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return Err("a host is required".to_string());
    }
    let kind = DeclaredKind::from_column(&kind).ok_or_else(|| format!("`{kind}` isn't a kind Hangar knows"))?;
    let scheme = match scheme.as_deref().map(str::trim).filter(|s| !s.is_empty()).unwrap_or("https") {
        s @ ("http" | "https") => s.to_string(),
        other => return Err(format!("`{other}` isn't a scheme — use http or https")),
    };

    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO forge_instances (host, kind, scheme, label, created_at) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(host) DO UPDATE SET kind = excluded.kind, scheme = excluded.scheme, label = excluded.label",
            rusqlite::params![host, kind.as_str(), scheme, label.unwrap_or_default().trim(), chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| e.to_string())?;
    }
    load_declared_hosts(&db)?;
    list_forge_instances_impl(&db)
}

/// Forgets a declaration, dropping the host back to the heuristics.
#[tauri::command]
pub fn delete_forge_instance(db: State<DbConnection>, host: String) -> Result<Vec<ForgeInstanceStatus>, String> {
    {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM forge_instances WHERE host = ?1", rusqlite::params![host.trim().to_ascii_lowercase()])
            .map_err(|e| e.to_string())?;
    }
    load_declared_hosts(&db)?;
    list_forge_instances_impl(&db)
}

/// Signs the instance's CLI in with a personal access token.
///
/// The token goes over stdin (`process_timeout::run_with_stdin`) and is never
/// stored by Hangar: `gh` and `glab` keep it in the OS keyring, which is where
/// it belongs — a database gets copied into a backup and a keyring does not.
#[tauri::command]
pub async fn authenticate_forge_instance<R: tauri::Runtime>(app: tauri::AppHandle<R>, host: String, token: String) -> Result<Vec<ForgeInstanceStatus>, String> {
    // `run_blocking` like its neighbours: the login is a CLI round trip over the
    // network, and as a synchronous command it held the UI thread for it.
    crate::blocking::run_blocking(move || {
        let host = host.trim().to_ascii_lowercase();
        if token.trim().is_empty() {
            return Err("a token is required".to_string());
        }
        forges::login_with_token(&host, token.trim())?;
        // The login just changed what every memo was taken under.
        forges::gitlab::forget_authenticated_hosts();
        list_forge_instances_impl(&app.state::<DbConnection>())
    })
    .await?
}

/// The repositories an instance's login can see.
#[tauri::command]
pub async fn list_instance_repos(host: String) -> Result<Vec<forges::InstanceRepo>, String> {
    // `run_blocking`, like every other command here that spawns: these are two
    // CLI round trips and would otherwise sit on the UI's async runtime.
    crate::blocking::run_blocking(move || forges::list_repos(&host)).await?
}

/// Clones a repository and registers it as a project in one step.
///
/// The registration reuses `projects::ensure_project_for_path`
/// (`commands/projects.rs:78`), which derives the name from the folder and
/// refuses to duplicate a path — so a repo cloned twice into the same place is
/// one project, exactly as if it had been added through the folder picker.
#[tauri::command]
pub async fn clone_instance_repo(app: AppHandle, clone_url: String, parent_dir: String, path: String) -> Result<super::projects::Project, String> {
    let parent = std::path::PathBuf::from(parent_dir.trim());
    if !parent.is_dir() {
        return Err(format!("{} isn't a directory", parent.display()));
    }
    // The last segment of `group/subgroup/project`, so a nested GitLab project
    // lands in `project/` rather than a folder with slashes in its name.
    let folder = path.rsplit('/').next().filter(|s| !s.is_empty()).ok_or_else(|| "that repository has no name".to_string())?;
    let destination = parent.join(folder);
    if destination.exists() {
        return Err(format!("{} already exists — move it or pick another folder", destination.display()));
    }

    let destination = crate::blocking::run_blocking(move || {
        let mut cmd = crate::providers::git_command();
        cmd.args(["clone", &clone_url, &destination.to_string_lossy()]);
        // Cloning a large repo can outlast the 25s default that suits an API
        // call, so this one gets its own bound rather than borrowing that one.
        let output = crate::process_timeout::run_with_timeout(cmd, std::time::Duration::from_secs(600))?;
        if !output.status.success() {
            return Err(format!("git clone failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        Ok::<_, String>(destination)
    })
    .await??;

    let db = app.state::<DbConnection>();
    super::projects::ensure_project_for_path(db, destination.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn projects() -> Vec<ForgeInstanceProject> {
        vec![ForgeInstanceProject { id: "p1".into(), name: "TMS-server".into() }]
    }

    /// Undeclared — what every host is until the user says otherwise, and the
    /// case most of these assert against.
    fn guessed(host: &str, placement: Placement) -> ForgeInstanceStatus {
        status_from(host.into(), projects(), placement, None)
    }

    fn gitlab_ssh(user: Option<&str>) -> Placement {
        let user = user.map(str::to_string);
        Placement::Gitlab { authed: false, ssh_user: user.clone(), viewer: user }
    }

    /// The live case, and the reason the SSH probe is worth two seconds.
    ///
    /// `git.technolabs.com.br` is a real GitLab on a vanity domain whose SSH key
    /// works — measured: `git ls-remote` lists branches, and the instance greets
    /// you as `@dsalinas`. Telling that user "Hangar can't tell what this host
    /// is" would be false, and telling them nothing at all is what left two
    /// projects silently missing. The row has to say the one true thing: git
    /// works, the API is what needs a token.
    #[test]
    fn a_vanity_gitlab_reached_over_ssh_explains_why_git_works_and_mrs_dont() {
        let status = guessed("git.technolabs.com.br", gitlab_ssh(Some("dsalinas")));

        assert_eq!(status.forge.as_deref(), Some("gitlab"), "the SSH greeting places it, with no token involved");
        assert_eq!(status.viewer_login.as_deref(), Some("dsalinas"), "and it is who you are there, without any API call");
        assert!(!status.authenticated, "but placing it is not the same as being able to call the API");
        let reason = status.reason.expect("it has to explain itself");
        assert!(reason.contains("@dsalinas"), "the identity the instance itself reported: {reason}");
        assert!(reason.contains("token"), "and the thing that's actually missing: {reason}");
        assert_eq!(status.login_command.as_deref(), Some("glab auth login --hostname git.technolabs.com.br"));
    }

    /// A self-hosted instance is authenticated per host, so the command has to
    /// carry `--hostname`. A bare `glab auth login` would log the user into
    /// gitlab.com and leave the instance they actually need untouched.
    #[test]
    fn a_gitlab_login_names_the_instance_not_just_the_cli() {
        let status = guessed("gitlab.acme.com", gitlab_ssh(None));

        assert_eq!(status.login_command.as_deref(), Some("glab auth login --hostname gitlab.acme.com"));
    }

    /// Nothing placed it at all — not its name, not `glab`, not SSH. Still has
    /// to name a next step rather than shrug.
    #[test]
    fn a_host_nothing_can_place_still_says_what_to_run() {
        let status = guessed("git.acme.com", Placement::Unknown);

        assert_eq!(status.forge, None, "unplaceable is not the same as unsupported");
        assert_eq!(status.login_command.as_deref(), Some("glab auth login --hostname git.acme.com"));
        assert!(status.reason.is_some_and(|r| r.contains("git.acme.com")));
    }

    /// **The `GitOnly` row is not a failure.** The user declared that this host
    /// is git and nothing more, so there is no session to be missing and no
    /// command to suggest — and saying otherwise would nag forever about a host
    /// that is working exactly as declared.
    #[test]
    fn a_git_only_instance_asks_for_nothing() {
        let declaration = Declaration { kind: DeclaredKind::GitOnly, scheme: "https".into(), label: "Gitea de casa".into() };
        let status = status_from("52.188.59.199:3000".into(), projects(), Placement::GitOnly, Some(declaration));

        assert!(status.git_only);
        assert_eq!(status.forge, None, "git-only means no adapter, which is not the same as an unknown one");
        assert_eq!(status.reason, None, "nothing is wrong with it");
        assert_eq!(status.login_command, None, "and so there is nothing to run");
        assert!(status.declared);
        assert_eq!(status.label, "Gitea de casa", "the label is the only thing distinguishing a bare IP from another");
    }

    /// A declaration is visible as such: the screen offers to correct a guess
    /// and must not invite second-guessing something the user already settled.
    #[test]
    fn a_row_says_whether_it_was_declared_or_guessed() {
        assert!(!guessed("gitlab.acme.com", gitlab_ssh(None)).declared);

        let declaration = Declaration { kind: DeclaredKind::Gitlab, scheme: "https".into(), label: "Technolabs".into() };
        let declared = status_from("git.technolabs.com.br".into(), projects(), gitlab_ssh(None), Some(declaration));
        assert!(declared.declared);
        assert_eq!(declared.label, "Technolabs");
    }

    /// Every unusable instance names a next step, and every usable one has
    /// nothing left to say — the same contract `list_requirements` holds row by
    /// row. A row with neither is a dead end.
    #[test]
    fn a_row_either_works_or_names_the_next_step() {
        let placements = [
            Placement::Github { authed: true, viewer: Some("killua15".into()) },
            Placement::Github { authed: false, viewer: None },
            Placement::Gitlab { authed: true, ssh_user: None, viewer: Some("dsalinas".into()) },
            gitlab_ssh(None),
            gitlab_ssh(Some("dsalinas")),
            Placement::Unknown,
        ];
        for placement in placements {
            let status = guessed("example.com", placement.clone());
            if status.authenticated {
                assert!(status.reason.is_none(), "{placement:?} works but still complains");
                assert!(status.login_command.is_none(), "{placement:?} works but still suggests a command");
            } else {
                assert!(status.reason.is_some(), "{placement:?} is blocked and won't say why");
                assert!(status.login_command.is_some(), "{placement:?} is blocked and won't say what to run");
            }
        }
        // `GitOnly` is deliberately outside that contract: it is neither
        // working-with-a-session nor blocked, and forcing it into either would
        // mean inventing a complaint or claiming a login it never needed.
        let git_only = status_from("example.com".into(), projects(), Placement::GitOnly, None);
        assert!(!git_only.authenticated && git_only.reason.is_none() && git_only.login_command.is_none());
    }

    /// The projects are the point: the user reads this to find out what stops
    /// working, not to be told a hostname is unreachable in the abstract.
    #[test]
    fn an_instance_carries_the_projects_resting_on_it() {
        let status = guessed("gitlab.com", Placement::Gitlab { authed: true, ssh_user: None, viewer: None });

        assert_eq!(status.projects, projects());
    }

    // -- grouping, against real checkouts -----------------------------------
    //
    // `group_by_host` rather than `list_forge_instances_impl`: the grouping is
    // what these assert, and going through the full command would spend a real
    // SSH round trip per host probing instances that aren't the point.

    /// A git repo with one `origin`, which is the only thing this reads.
    fn repo_with_origin(label: &str, url: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-test-forgestatus-{label}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for args in [vec!["init", "-q"], vec!["remote", "add", "origin", url]] {
            std::process::Command::new("git").args(&args).current_dir(&dir).output().unwrap();
        }
        dir
    }

    fn row(id: &str, name: &str, path: &std::path::Path) -> (String, String, String) {
        (id.to_string(), name.to_string(), path.to_string_lossy().to_string())
    }

    /// Two projects on the same self-hosted instance produce **one** group
    /// carrying both — not two, which is what makes the login probes cost once
    /// per instance instead of once per project. The live case: `tms-server` and
    /// `simulation-server` both sit on `git.technolabs.com.br`.
    #[test]
    fn projects_sharing_an_instance_are_grouped_once() {
        let a = repo_with_origin("a", "git@git.technolabs.com.br:tms-3.0/TMS-server.git");
        let b = repo_with_origin("b", "git@git.technolabs.com.br:tms-3.0/TMS-simulation-server.git");

        let grouped = group_by_host(vec![row("p1", "tms-server", &a), row("p2", "simulation-server", &b)]);

        assert_eq!(grouped.len(), 1, "one group per instance: {grouped:?}");
        assert_eq!(grouped[0].0, "git.technolabs.com.br");
        assert_eq!(grouped[0].1.len(), 2, "both projects hang off it: {:?}", grouped[0].1);

        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    /// Different instances stay apart even when the forge is the same — the
    /// whole reason this is keyed by host.
    #[test]
    fn two_instances_of_the_same_forge_stay_apart() {
        let hosted = repo_with_origin("hosted", "https://gitlab.com/group/proj.git");
        let selfhosted = repo_with_origin("self", "git@gitlab.acme.com:group/proj.git");

        let grouped = group_by_host(vec![row("p1", "hosted", &hosted), row("p2", "self-hosted", &selfhosted)]);

        assert_eq!(grouped.len(), 2, "gitlab.com and a self-hosted instance are different servers: {grouped:?}");

        std::fs::remove_dir_all(&hosted).ok();
        std::fs::remove_dir_all(&selfhosted).ok();
    }

    // -- declarations round-tripping through the database --------------------

    fn db_with_schema() -> DbConnection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        DbConnection(std::sync::Mutex::new(conn))
    }

    /// **What a restart depends on.** `load_declared_hosts` is what `lib.rs`
    /// calls at startup, and if it doesn't reach the registry, every launch
    /// begins by guessing at hosts the user already settled — once per launch,
    /// on the first poll pass, which is the hardest kind of bug to catch later.
    #[test]
    fn a_stored_declaration_reaches_the_registry_at_startup() {
        let _guard = forges::lock_declarations_for_test();
        let db = db_with_schema();
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO forge_instances (host, kind, scheme, label, created_at)
                 VALUES ('git.acme.com','github','https','Acme','now'), ('10.0.0.5:3000','git_only','http','Gitea','now')",
                [],
            )
            .unwrap();
        }

        let before = forges::declared_kind("git.acme.com");
        load_declared_hosts(&db).unwrap();

        assert_eq!(forges::declared_kind("git.acme.com"), Some(DeclaredKind::Github), "a vanity-domain GHE no heuristic can reach");
        assert_eq!(forges::declared_kind("10.0.0.5:3000"), Some(DeclaredKind::GitOnly), "port included — it is part of the identity");
        assert!(forges::declarations_loaded());

        // Process-wide state: put back whatever the rest of the suite had.
        let restore = before.map(|k| ("git.acme.com".to_string(), k)).into_iter().collect();
        forges::set_declared_hosts(restore);
    }

    /// A declared instance shows up before anything has been cloned from it —
    /// otherwise declaring one, then browsing it, would be impossible in that
    /// order.
    #[test]
    fn a_declared_instance_appears_with_no_projects_on_it_yet() {
        let _guard = forges::lock_declarations_for_test();
        let db = db_with_schema();
        {
            let conn = db.0.lock().unwrap();
            conn.execute(
                "INSERT INTO forge_instances (host, kind, scheme, label, created_at) VALUES ('10.0.0.5:3000','git_only','http','Gitea','now')",
                [],
            )
            .unwrap();
        }
        load_declared_hosts(&db).unwrap();

        let instances = list_forge_instances_impl(&db).unwrap();

        let found = instances.iter().find(|i| i.host == "10.0.0.5:3000").expect("declared, so it must be listed");
        assert!(found.projects.is_empty());
        assert!(found.git_only, "and it is the declaration that says so, with no probing");
        assert_eq!(found.label, "Gitea");

        forges::set_declared_hosts(Default::default());
    }

    /// A project that isn't a git repo waits on no instance, so it must not
    /// invent a group. Hangar has several of those — "copy"-only projects.
    #[test]
    fn a_project_with_no_remote_produces_no_instance() {
        let plain = std::env::temp_dir().join(format!("hangar-test-forgestatus-plain-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&plain).unwrap();

        assert!(group_by_host(vec![row("p1", "not a repo", &plain)]).is_empty());

        std::fs::remove_dir_all(&plain).ok();
    }
}
