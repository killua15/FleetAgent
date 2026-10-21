//! GitLab, driven through the `glab` CLI.
//!
//! **REST, not GraphQL**, which is the opposite of the GitHub adapter and is a
//! deliberate inversion. GitLab's GraphQL enforces a query-complexity budget
//! that `notes(first: 100)` across fifty merge requests blows straight through,
//! and self-hosted instances lag the published schema by versions — a field
//! that works on gitlab.com can 400 on `gitlab.acme.com`, which is exactly the
//! deployment this adapter exists for. The cost argument that justified
//! GitHub's move to GraphQL doesn't apply here either: with
//! [`PendingComments`] deferring the notes call, the steady state is one
//! `glab` spawn per repo per pass, the same as GitHub.

use super::{CreatedChangeRequest, NewChangeRequest, PendingComments, PrFacts, RawPrComment, RepoPrSnapshot, RepoRef, SnapshotPr};
use crate::providers;
use serde_json::Value;
use std::process::Command;

/// One `glab` invocation, with the instance always named explicitly.
///
/// `--hostname` on every call rather than letting `glab` infer it: the poller
/// runs with no working directory at all, and `glab`'s `GITLAB_HOST`
/// environment variable is a known-broken path (gitlab-org/cli#1304), so the
/// flag is the only channel that can be relied on.
///
/// Timed out for the same reason every `gh` call is: a hung CLI would block
/// its `run_blocking` thread forever.
fn glab_output(host: &str, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new(providers::resolve_binary("glab"));
    cmd.args(args).args(["--hostname", host]);
    crate::process_timeout::run_with_timeout(cmd, crate::process_timeout::DEFAULT_TIMEOUT).map_err(|e| format!("glab {}: {e}", args.join(" ")))
}

/// A `GET` against the instance's REST API, parsed.
fn glab_api_json(host: &str, endpoint: &str) -> Result<Value, String> {
    let output = glab_output(host, &["api", endpoint])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // A 404 from GitLab is usually not a missing project — it hides what
        // the token may not read rather than admitting it exists. Saying so
        // beats repeating "not found".
        if stderr.contains("404") {
            return Err(format!("{endpoint} isn't visible to the account `glab` is logged in to on {host} — check `glab auth status`"));
        }
        return Err(format!("glab api {endpoint} failed: {stderr}"));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("glab api {endpoint} returned unparseable JSON: {e}"))
}

/// Asked of the instance the repo is on. The capability's own login check can
/// only ask about some instance, and asking gitlab.com refused every merge
/// request on a self-hosted GitLab for someone with no gitlab.com token.
pub fn available_and_authed(host: &str) -> Result<(), String> {
    crate::capabilities::Capability::MergeRequests.check_offline().map_err(|b| b.message())?;
    if !is_authenticated(host) {
        return Err(format!("`glab` isn't logged in to {host} — run `glab auth login --hostname {host}`"));
    }
    Ok(())
}

/// Whether `glab` is logged in to this instance at all.
///
/// Also the last-resort forge probe for a host whose name gives nothing away
/// (`git.acme.com`): a host someone has run `glab auth login` against *is* a
/// GitLab host, by definition. Memoized per host for the process — the answer
/// costs a whole process spawn and cannot change without the user running a
/// login, at which point they relaunch anyway.
pub fn is_authenticated(host: &str) -> bool {
    if let Ok(cached) = AUTHED_HOSTS.lock() {
        if let Some(known) = cached.get(host) {
            return *known;
        }
    }
    let answer = glab_output(host, &["auth", "status"]).is_ok_and(|o| o.status.success());
    if let Ok(mut cached) = AUTHED_HOSTS.lock() {
        cached.insert(host.to_string(), answer);
    }
    answer
}

static AUTHED_HOSTS: std::sync::Mutex<std::collections::BTreeMap<String, bool>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The logged-in user's username on this instance.
///
/// GitHub gets this free inside its one GraphQL document; GitLab needs its own
/// call, so it is memoized per host: it is the same answer every pass, and
/// every pass would otherwise pay a process spawn for it.
fn current_username(host: &str) -> Option<String> {
    if let Ok(cached) = VIEWER_LOGINS.lock() {
        if let Some(known) = cached.get(host) {
            return known.clone();
        }
    }
    let answer = glab_api_json(host, "user").ok().and_then(|u| u.get("username").and_then(|v| v.as_str()).map(|s| s.to_string()));
    if let Ok(mut cached) = VIEWER_LOGINS.lock() {
        cached.insert(host.to_string(), answer.clone());
    }
    answer
}

static VIEWER_LOGINS: std::sync::Mutex<std::collections::BTreeMap<String, Option<String>>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Who the API says you are on this instance — the same memoized answer the
/// snapshot uses to decide whose merge requests are whose, exposed so the
/// instance screen can show it.
pub fn viewer_login(host: &str) -> Option<String> {
    current_username(host)
}

/// Hands `glab` a token on stdin, so it never reaches argv or a log.
///
/// `--stdin` is `glab`'s own affordance for exactly this. The alternative Hangar
/// used first — opening a terminal with `glab auth login` typed into it — still
/// exists for whoever prefers the browser flow, but it cannot be completed
/// without leaving the app, and it cannot be scripted.
pub fn login_with_token(host: &str, token: &str) -> Result<(), String> {
    let mut cmd = Command::new(providers::resolve_binary("glab"));
    cmd.args(["auth", "login", "--hostname", host, "--stdin"]);
    let output = crate::process_timeout::run_with_stdin(cmd, token, crate::process_timeout::DEFAULT_TIMEOUT)?;
    if !output.status.success() {
        // `glab`'s own words, which name the real reason (a scope the token
        // lacks, an instance that refused it) far better than anything here
        // could guess.
        return Err(format!("glab auth login failed on {host}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    // The memos were taken while this host was logged out.
    forget_authenticated_hosts();
    Ok(())
}

/// The projects this token can see on the instance, newest activity first.
pub fn list_repos(host: &str) -> Result<Vec<super::InstanceRepo>, String> {
    let json = glab_api_json(host, "projects?membership=true&order_by=last_activity_at&per_page=100")?;
    Ok(json
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let path = p.get("path_with_namespace")?.as_str()?.to_string();
            Some(super::InstanceRepo {
                name: p.get("name").and_then(|v| v.as_str()).unwrap_or(&path).to_string(),
                path,
                description: p.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                web_url: p.get("web_url").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                // ssh first: it is what a self-hosted instance is nearly always
                // set up for, and it is the one that needs no token to clone.
                clone_url: p
                    .get("ssh_url_to_repo")
                    .or_else(|| p.get("http_url_to_repo"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
            })
        })
        .filter(|r| !r.clone_url.is_empty())
        .collect())
}

/// Throws away both memos, for after the user has run a login.
///
/// The comment on [`is_authenticated`] says the answer "cannot change without
/// the user running a login, at which point they relaunch anyway" — that
/// assumption stops holding the moment Hangar itself offers the login. Without
/// this, someone authenticates in the terminal Hangar opened for them and the
/// app keeps insisting they haven't, which reads as "I did what it said and
/// nothing happened". `recheck_requirements` clears the binary cache for
/// exactly the same reason.
///
/// `VIEWER_LOGINS` goes too: a fresh login can be a *different user* on the
/// same instance, and a stale viewer login is what decides whether a merge
/// request counts as yours.
pub fn forget_authenticated_hosts() {
    if let Ok(mut cached) = AUTHED_HOSTS.lock() {
        cached.clear();
    }
    if let Ok(mut cached) = VIEWER_LOGINS.lock() {
        cached.clear();
    }
    if let Ok(mut cached) = SSH_IDENTITIES.lock() {
        cached.clear();
    }
}

/// Who the user's SSH key is on this instance, according to the instance itself.
///
/// GitLab answers `ssh -T git@host` with `Welcome to GitLab, @user!`, and that
/// one line settles two things a token would otherwise be needed for: the host
/// **is** a GitLab, and this is who you are on it.
///
/// What it deliberately does **not** do is replace a token. Measured against a
/// real self-hosted instance: the SSH key authenticates git perfectly
/// (`git ls-remote` lists branches) while `GET /api/v4/user` answers **401** —
/// GitLab's REST API takes a PAT or OAuth token and never an SSH key. Merge
/// requests are listed through that API, so this can improve the diagnosis and
/// cannot skip the login.
///
/// `BatchMode=yes` matters: without it `ssh` can sit waiting for a passphrase or
/// a host-key confirmation with nobody at the keyboard, and a hung process is
/// exactly what [`crate::process_timeout`] exists to stop. Memoized because it
/// costs a full round trip — measured at ~2s — which is also why no hot path
/// calls it; only the instance diagnosis does, when the user asks.
pub fn ssh_identity(host: &str) -> Option<String> {
    if let Ok(cached) = SSH_IDENTITIES.lock() {
        if let Some(known) = cached.get(host) {
            return known.clone();
        }
    }
    let mut cmd = Command::new("ssh");
    cmd.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "StrictHostKeyChecking=accept-new", &format!("git@{host}")]);
    let answer = crate::process_timeout::run_with_timeout(cmd, crate::process_timeout::DEFAULT_TIMEOUT)
        .ok()
        // The greeting arrives on either stream depending on the instance, so
        // both get read rather than guessing which.
        .and_then(|o| parse_ssh_greeting(&String::from_utf8_lossy(&o.stdout)).or_else(|| parse_ssh_greeting(&String::from_utf8_lossy(&o.stderr))));
    if let Ok(mut cached) = SSH_IDENTITIES.lock() {
        cached.insert(host.to_string(), answer.clone());
    }
    answer
}

static SSH_IDENTITIES: std::sync::Mutex<std::collections::BTreeMap<String, Option<String>>> = std::sync::Mutex::new(std::collections::BTreeMap::new());

/// The username out of `Welcome to GitLab, @user!`, or `None` for anything that
/// isn't a GitLab saying hello.
///
/// The "GitLab" check is the part that carries weight: any SSH host will answer
/// something, and only this greeting means the host is a GitLab.
fn parse_ssh_greeting(output: &str) -> Option<String> {
    let line = output.lines().find(|l| l.contains("Welcome to GitLab"))?;
    let after_at = line.split_once('@')?.1;
    let user = after_at.trim_end().trim_end_matches('!').trim();
    (!user.is_empty()).then(|| user.to_string())
}

/// `group/subgroup/project` → `group%2Fsubgroup%2Fproject`, for `projects/:id`.
///
/// A plain slash swap and nothing else, because GitLab restricts every path
/// segment to `[a-zA-Z0-9_.-]` — so there is provably nothing else in a valid
/// project path that needs escaping, and this needs no new dependency.
fn encoded_path(repo: &RepoRef) -> String {
    repo.path().replace('/', "%2F")
}

/// GitLab's state vocabulary, mapped onto the `state` column's.
///
/// `locked` is a merge in progress — the merge request is still open and still
/// accruing comments, so treating it as anything else would make a live review
/// vanish from the inbox mid-merge.
pub(crate) fn map_state(gitlab_state: &str) -> String {
    match gitlab_state {
        "merged" => "merged",
        "closed" => "closed",
        _ => "open",
    }
    .to_string()
}

/// Whether this merge request is waiting on the given user.
///
/// GitLab carries `reviewers` inline on every merge request, so unlike GitHub
/// this needs no second search query — but it does mean the answer is computed
/// here rather than handed down by the server.
pub(crate) fn is_awaiting_my_review(mr: &Value, me: Option<&str>) -> bool {
    let Some(me) = me else { return false };
    let has = |field: &str| {
        mr.get(field)
            .and_then(|v| v.as_array())
            .is_some_and(|list| list.iter().any(|u| u.get("username").and_then(|v| v.as_str()) == Some(me)))
    };
    has("reviewers") || has("assignees")
}

/// `repo` is threaded in rather than derived from the payload because it is
/// what the deferred comment fetch will need to address this merge request
/// later — GitLab's own response carries the numeric `project_id`, not the
/// path, and `projects/:id` wants the path.
pub(crate) fn parse_mr(mr: &Value, repo: &RepoRef) -> Option<SnapshotPr> {
    let number = mr.get("iid").and_then(|v| v.as_i64())?;
    Some(SnapshotPr {
        number,
        url: mr.get("web_url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        title: mr.get("title").and_then(|v| v.as_str()).map(|s| s.to_string()),
        created_at: mr.get("created_at").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        updated_at: mr.get("updated_at").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        state: map_state(mr.get("state").and_then(|v| v.as_str()).unwrap_or("opened")),
        author_login: mr.pointer("/author/username").and_then(|v| v.as_str()).map(|s| s.to_string()),
        assignees: mr
            .get("assignees")
            .and_then(|v| v.as_array())
            .map(|list| list.iter().filter_map(|a| a.get("username").and_then(|v| v.as_str()).map(|s| s.to_string())).collect())
            .unwrap_or_default(),
        // GitLab's names are the other way round from GitHub's: `source` is
        // where the work is, `target` is what it merges into.
        head_branch: mr.get("source_branch").and_then(|v| v.as_str()).map(|s| s.to_string()),
        base_branch: mr.get("target_branch").and_then(|v| v.as_str()).map(|s| s.to_string()),
        comments: PendingComments::Deferred { repo: repo.clone(), number },
    })
}

/// Turns one merge request's notes into comments, oldest first.
///
/// Three rules earn their place here:
///
/// - **`system: true` notes are dropped.** GitLab records "assigned to @x",
///   "changed title", "added 3 commits" and every label edit as notes in the
///   same list as real review comments. Left in, they would bury the actual
///   feedback and fire a notification for every housekeeping event.
/// - **Empty bodies are dropped**, matching the GitHub adapter's treatment of
///   a bare approval: nothing to act on.
/// - **`new_line` falls back to `old_line`.** A comment on a deleted line has
///   only the old side, the same situation GitHub's `originalLine` covers.
pub(crate) fn parse_notes(notes: &Value) -> Vec<RawPrComment> {
    let mut out: Vec<RawPrComment> = notes
        .as_array()
        .into_iter()
        .flatten()
        .filter(|n| !n.get("system").and_then(|v| v.as_bool()).unwrap_or(false))
        .filter_map(|n| {
            let body = n.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if body.trim().is_empty() {
                return None;
            }
            Some(RawPrComment {
                external_id: n.get("id").and_then(|v| v.as_i64()).map(|id| id.to_string()).unwrap_or_default(),
                author: n.pointer("/author/username").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                body,
                created_at: n.get("created_at").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                path: n
                    .pointer("/position/new_path")
                    .or_else(|| n.pointer("/position/old_path"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                line: n.pointer("/position/new_line").and_then(|v| v.as_i64()).or_else(|| n.pointer("/position/old_line").and_then(|v| v.as_i64())),
            })
        })
        .collect();
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

/// How many notes one merge request contributes. 100 is GitLab's own ceiling
/// for `per_page`, and the request asks for them **newest first** and reverses
/// here: on a long-running merge request an oldest-first page would spend the
/// whole budget on history and hide the comments that just arrived — which are
/// the only ones this feature exists to surface. Same shape as the GitHub
/// adapter's `comments(last: 50)`.
const NOTES_PER_PAGE: &str = "100";

pub fn fetch_comments(repo: &RepoRef, number: i64) -> Result<Vec<RawPrComment>, String> {
    let endpoint = format!("projects/{}/merge_requests/{number}/notes?per_page={NOTES_PER_PAGE}&sort=desc", encoded_path(repo));
    Ok(parse_notes(&glab_api_json(&repo.host, &endpoint)?))
}

pub fn fetch_repo_snapshot(repo: &RepoRef, limit: usize) -> Result<RepoPrSnapshot, String> {
    let endpoint = format!("projects/{}/merge_requests?state=opened&order_by=updated_at&sort=desc&per_page={limit}", encoded_path(repo));
    let json = glab_api_json(&repo.host, &endpoint)?;
    let viewer_login = current_username(&repo.host);

    let mut review_requested = std::collections::HashSet::new();
    let mut prs = Vec::new();
    for mr in json.as_array().into_iter().flatten() {
        let Some(parsed) = parse_mr(mr, repo) else { continue };
        if is_awaiting_my_review(mr, viewer_login.as_deref()) {
            review_requested.insert(parsed.number);
        }
        prs.push(parsed);
    }
    Ok(RepoPrSnapshot { viewer_login, review_requested, prs })
}

pub fn fetch_single_pr_facts(repo: &RepoRef, number: i64) -> Result<PrFacts, String> {
    let endpoint = format!("projects/{}/merge_requests/{number}", encoded_path(repo));
    let json = glab_api_json(&repo.host, &endpoint)?;
    let parsed = parse_mr(&json, repo).ok_or_else(|| "glab returned a merge request with no iid".to_string())?;
    Ok(PrFacts {
        state: parsed.state,
        updated_at: parsed.updated_at,
        comments: PendingComments::Deferred { repo: repo.clone(), number },
        viewer_login: current_username(&repo.host),
    })
}

pub fn fetch_head_branch(repo: &RepoRef, number: i64) -> Result<String, String> {
    let endpoint = format!("projects/{}/merge_requests/{number}", encoded_path(repo));
    let json = glab_api_json(&repo.host, &endpoint)?;
    json.get("source_branch")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "glab did not return a source_branch".to_string())
}

pub fn default_branch(repo: &RepoRef) -> Option<String> {
    let json = glab_api_json(&repo.host, &format!("projects/{}", encoded_path(repo))).ok()?;
    json.get("default_branch").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string())
}

/// Opens a merge request.
///
/// Through `glab api --method POST` rather than `glab mr create`, which is
/// interactive by default: it prompts for the target branch and for "remove
/// source branch?", and returns prose. A hidden prompt is exactly the hang
/// that `process_timeout` and `GIT_TERMINAL_PROMPT=0` exist elsewhere in this
/// codebase to prevent, and the REST endpoint answers with `{iid, web_url}`
/// deterministically. The trade-off, stated plainly: this bypasses GitLab's
/// merge-request description templates, since the body is sent verbatim.
pub fn create_change_request(repo: &RepoRef, req: &NewChangeRequest) -> Result<CreatedChangeRequest, String> {
    let endpoint = format!("projects/{}/merge_requests", encoded_path(repo));
    let output = glab_output(
        &repo.host,
        &[
            "api",
            "--method",
            "POST",
            &endpoint,
            "-f",
            &format!("source_branch={}", req.head),
            "-f",
            &format!("target_branch={}", req.base),
            "-f",
            &format!("title={}", req.title),
            "-f",
            &format!("description={}", req.body),
        ],
    )?;
    if !output.status.success() {
        return Err(format!("glab merge request create failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("glab returned unparseable JSON creating the merge request: {e}"))?;
    let number = json.get("iid").and_then(|v| v.as_i64()).ok_or_else(|| "glab created a merge request with no iid".to_string())?;
    let url = json.get("web_url").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Ok(CreatedChangeRequest { url, number })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> RepoRef {
        RepoRef { forge: super::super::Forge::Gitlab, host: "gitlab.com".into(), namespace: "group/subgroup".into(), name: "project".into() }
    }

    #[test]
    fn a_nested_project_path_is_encoded_for_the_projects_endpoint() {
        assert_eq!(encoded_path(&repo()), "group%2Fsubgroup%2Fproject");
    }

    /// A merge request in the shape `GET /projects/:id/merge_requests` returns
    /// (per GitLab's REST documentation — this is not a capture from a live
    /// instance, see the module tests' note in `fixtures/`).
    fn mr_json() -> Value {
        serde_json::from_str(
            r#"{
              "id": 155016530, "iid": 42, "project_id": 15513260,
              "title": "Fix billing rounding", "description": "the body",
              "state": "opened",
              "created_at": "2026-01-01T00:00:00.000Z", "updated_at": "2026-01-02T00:00:00.000Z",
              "target_branch": "main", "source_branch": "feat/billing",
              "author": {"id": 1, "username": "alice", "name": "Alice"},
              "assignees": [{"id": 2, "username": "killua15"}],
              "reviewers": [{"id": 3, "username": "bob"}],
              "web_url": "https://gitlab.com/group/subgroup/project/-/merge_requests/42"
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn parse_mr_reads_the_iid_the_branches_and_the_author() {
        let parsed = parse_mr(&mr_json(), &repo()).unwrap();
        assert_eq!(parsed.number, 42, "GitLab's project-scoped iid is the number, not the global id");
        assert_eq!(parsed.state, "open", "GitLab says `opened`; the column stores `open`");
        assert_eq!(parsed.author_login.as_deref(), Some("alice"));
        assert_eq!(parsed.assignees, vec!["killua15".to_string()]);
        assert_eq!(parsed.head_branch.as_deref(), Some("feat/billing"), "GitLab's source_branch is GitHub's head");
        assert_eq!(parsed.base_branch.as_deref(), Some("main"), "GitLab's target_branch is GitHub's base");
    }

    /// A merge in progress is still open and still accruing comments. Reading
    /// `locked` as anything else would drop a live review out of the inbox at
    /// the exact moment someone is merging it.
    #[test]
    fn a_locked_merge_request_is_still_open() {
        assert_eq!(map_state("locked"), "open");
        assert_eq!(map_state("opened"), "open");
        assert_eq!(map_state("merged"), "merged");
        assert_eq!(map_state("closed"), "closed");
    }

    #[test]
    fn a_merge_request_is_awaiting_me_when_i_am_a_reviewer_or_an_assignee() {
        let mr = mr_json();
        assert!(is_awaiting_my_review(&mr, Some("bob")), "a reviewer is being asked");
        assert!(is_awaiting_my_review(&mr, Some("killua15")), "so is an assignee");
        assert!(!is_awaiting_my_review(&mr, Some("carol")), "a bystander is not");
        assert!(!is_awaiting_my_review(&mr, None), "and with no known login, nothing is claimed");
    }

    /// Notes as `GET /projects/:id/merge_requests/:iid/notes` returns them:
    /// a system note, a plain comment, an inline `DiffNote`, one on a deleted
    /// line (`new_line: null`), and an empty-bodied one.
    fn notes_json() -> Value {
        serde_json::from_str(
            r#"[
              {"id": 301, "body": "assigned to @killua15", "system": true,
               "author": {"username": "alice"}, "created_at": "2026-01-01T00:00:00Z", "type": null},
              {"id": 302, "body": "please fix the typo", "system": false,
               "author": {"username": "alice"}, "created_at": "2026-01-01T00:01:00Z", "type": null},
              {"id": 303, "body": "this lookup can return null", "system": false,
               "author": {"username": "dave"}, "created_at": "2026-01-01T00:02:00Z", "type": "DiffNote",
               "position": {"old_path": "src/billing.rs", "new_path": "src/billing.rs",
                            "position_type": "text", "old_line": null, "new_line": 40}},
              {"id": 304, "body": "and this line is gone", "system": false,
               "author": {"username": "dave"}, "created_at": "2026-01-01T00:03:00Z", "type": "DiffNote",
               "position": {"old_path": "src/old.rs", "new_path": "src/old.rs",
                            "position_type": "text", "old_line": 62, "new_line": null}},
              {"id": 305, "body": "   ", "system": false,
               "author": {"username": "bob"}, "created_at": "2026-01-01T00:04:00Z", "type": null}
            ]"#,
        )
        .unwrap()
    }

    /// **The flood guard.** GitLab records every assignment, title edit, label
    /// change and pushed commit as a note in the same list as real review
    /// feedback. Left in, each one would persist a row and fire a
    /// notification for something nobody wrote.
    #[test]
    fn parse_notes_drops_system_notes() {
        let comments = parse_notes(&notes_json());
        assert!(!comments.iter().any(|c| c.external_id == "301"), "the system note must not reach the inbox: {comments:?}");
    }

    #[test]
    fn parse_notes_drops_empty_bodies_and_orders_oldest_first() {
        let comments = parse_notes(&notes_json());
        let ids: Vec<&str> = comments.iter().map(|c| c.external_id.as_str()).collect();
        assert_eq!(ids, vec!["302", "303", "304"], "the whitespace-only note is dropped and the rest run oldest first");
    }

    #[test]
    fn parse_notes_reads_the_diff_position_as_path_and_line() {
        let comments = parse_notes(&notes_json());
        let inline = comments.iter().find(|c| c.external_id == "303").unwrap();
        assert_eq!(inline.path.as_deref(), Some("src/billing.rs"));
        assert_eq!(inline.line, Some(40));

        let deleted = comments.iter().find(|c| c.external_id == "304").unwrap();
        assert_eq!(deleted.line, Some(62), "a comment on a deleted line has only the old side — the same case GitHub's originalLine covers");
    }

    #[test]
    fn a_plain_comment_carries_no_file_or_line() {
        let comments = parse_notes(&notes_json());
        let plain = comments.iter().find(|c| c.external_id == "302").unwrap();
        assert_eq!(plain.path, None);
        assert_eq!(plain.line, None);
    }
}

/// Tests that spawn a real `glab` against a real instance.
///
/// The fixtures above are written from GitLab's published REST shapes rather
/// than captured from a live server, which is honest but weaker than the
/// GitHub adapter's — these are what close that gap, and the first one to run
/// should have its output pasted back into `mr_json`/`notes_json`.
///
/// Run with `cargo test -- --ignored`, having done `glab auth login`.
#[cfg(test)]
mod live_tests {
    use super::*;

    fn repo_from_env(var: &str) -> Option<RepoRef> {
        let spec = std::env::var(var).ok()?;
        let (host, path) = spec.split_once('/')?;
        let (namespace, name) = path.rsplit_once('/')?;
        Some(RepoRef { forge: super::super::Forge::Gitlab, host: host.to_string(), namespace: namespace.to_string(), name: name.to_string() })
    }

    /// End-to-end read against a real project, including a nested group if the
    /// one named has subgroups.
    #[test]
    #[ignore = "spawns a real `glab`; set HANGAR_GITLAB_TEST_REPO=gitlab.com/group/project and run `glab auth login` first"]
    fn a_real_snapshot_comes_back_with_usable_merge_requests() {
        let repo = repo_from_env("HANGAR_GITLAB_TEST_REPO").expect("set HANGAR_GITLAB_TEST_REPO=<host>/<namespace>/<project>");
        let snapshot = fetch_repo_snapshot(&repo, 50).expect("the snapshot must come back");

        println!("viewer: {:?}, {} open merge request(s)", snapshot.viewer_login, snapshot.prs.len());
        assert!(snapshot.viewer_login.is_some(), "`glab api user` must resolve, or every merge request classifies as `other`");
        for mr in &snapshot.prs {
            assert!(mr.number > 0, "an iid of 0 means the parse missed it: {mr:?}");
            assert!(mr.head_branch.is_some(), "without source_branch a review has no branch to check out: {mr:?}");
        }
    }

    /// Proves `--hostname` actually targets the instance rather than falling
    /// back to gitlab.com — the whole self-hosted story rests on it.
    #[test]
    #[ignore = "spawns a real `glab` against a self-hosted instance; set HANGAR_GITLAB_TEST_SELFHOSTED=<host>/<namespace>/<project>"]
    fn a_self_hosted_instance_is_reached_by_hostname() {
        let repo = repo_from_env("HANGAR_GITLAB_TEST_SELFHOSTED").expect("set HANGAR_GITLAB_TEST_SELFHOSTED=<host>/<namespace>/<project>");
        assert_ne!(repo.host, "gitlab.com", "this test only proves anything against a non-default host");
        assert!(is_authenticated(&repo.host), "`glab auth login --hostname {}` first", repo.host);

        let snapshot = fetch_repo_snapshot(&repo, 50).expect("the self-hosted snapshot must come back");
        println!("{}: {} open merge request(s)", repo.host, snapshot.prs.len());
    }

    /// The write path. **Leaves a real merge request open** — hence the second
    /// env var, so it can't fire from a bare `--ignored` run.
    #[test]
    #[ignore = "creates a REAL merge request; needs HANGAR_GITLAB_TEST_REPO and HANGAR_GITLAB_TEST_BRANCH (already pushed)"]
    fn creating_a_merge_request_returns_its_iid_and_url() {
        let repo = repo_from_env("HANGAR_GITLAB_TEST_REPO").expect("set HANGAR_GITLAB_TEST_REPO=<host>/<namespace>/<project>");
        let head = std::env::var("HANGAR_GITLAB_TEST_BRANCH").expect("set HANGAR_GITLAB_TEST_BRANCH to a branch already pushed to the project");
        let base = default_branch(&repo).unwrap_or_else(|| "main".to_string());

        let created = create_change_request(&repo, &NewChangeRequest { title: "Hangar integration test", body: "_opened by a test_", base: &base, head: &head })
            .expect("the merge request must be created");

        println!("opened {} (!{})", created.url, created.number);
        assert!(created.number > 0);
        assert!(created.url.contains("merge_requests"), "got: {}", created.url);
    }
}
