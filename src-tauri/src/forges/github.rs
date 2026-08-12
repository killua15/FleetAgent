//! GitHub, driven through the `gh` CLI.
//!
//! Everything here used to live in `commands::pull_requests`; it moved so that
//! the orchestration above it — the check state machine, the persistence, the
//! poller — stopped knowing which forge it was talking to. The behaviour is
//! unchanged, and the tests that came with it are the proof.
//!
//! No GitHub App and no webhook: Hangar is a desktop app with no public
//! endpoint, so the model is polling while the app is open plus a manual
//! "check now".

use super::{CreatedChangeRequest, NewChangeRequest, PendingComments, PrFacts, RawPrComment, RepoPrSnapshot, RepoRef, SnapshotPr};
use crate::providers;
use serde_json::Value;
use std::path::Path;
use std::process::Command;

/// Every `gh` call in the app goes through here — a hung `gh` process (dead
/// network, an interactive re-auth prompt) would otherwise block whichever
/// `run_blocking` thread called it forever, see `process_timeout` for why that
/// matters.
pub fn gh_output(cwd: Option<&Path>, args: &[&str]) -> Result<std::process::Output, String> {
    let mut cmd = Command::new(providers::resolve_binary("gh"));
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    crate::process_timeout::run_with_timeout(cmd, crate::process_timeout::DEFAULT_TIMEOUT).map_err(|e| format!("gh {}: {e}", args.join(" ")))
}

/// Clear, actionable error instead of letting a bare `gh pr create` failure
/// (missing binary or expired login) surface as a cryptic exit-code message.
pub fn available_and_authed() -> Result<(), String> {
    // One line, because the knowledge lives where every feature can ask the
    // same question: `gh` present *and* logged in, since installed-but-not-
    // authenticated fails exactly as hard as absent.
    crate::capabilities::Capability::PullRequests.check().map_err(|b| b.message())
}

/// The PR fields every GraphQL query here asks for, as a fragment shared by
/// the repo-wide and single-PR queries so the two can never drift.
///
/// **The `id` choices are a compatibility contract, not a preference.**
/// `external_id` values already in the database came from `gh pr view --json
/// comments,reviews` (GraphQL node ids like `PRR_kwDO…`, verified identical to
/// what `id` returns here) and from the REST inline-comments endpoint (numeric
/// ids). So: `id` for comments and reviews, `databaseId` for review-thread
/// comments. Getting this wrong wouldn't fail loudly — it would make every
/// comment ever seen look new and flood the inbox on the first pass.
const PR_FIELDS_FRAGMENT: &str = r#"
fragment prFields on PullRequest {
  number url title createdAt updatedAt state
  author { login }
  assignees(first: 10) { nodes { login } }
  headRefName baseRefName
  comments(last: 50) { nodes { id body createdAt author { login } } }
  reviews(last: 50) { nodes { id body submittedAt author { login } } }
  reviewThreads(last: 50) { nodes { comments(first: 20) { nodes { databaseId body createdAt path line originalLine author { login } } } } }
}"#;

/// Pulls the three comment sources out of one PR's GraphQL node and merges
/// them into one list, oldest first.
///
/// Same rules as the two REST parsers this replaced: bodyless reviews (a bare
/// "approve" with nothing to act on) are dropped, and an inline comment's
/// `line` falls back to `originalLine`, which is what GitHub leaves once the
/// commented line has scrolled out of the current diff.
fn parse_graphql_pr_comments(pr: &Value) -> Vec<RawPrComment> {
    let mut out = Vec::new();

    let mut push_simple = |nodes: Option<&Vec<Value>>, date_field: &str| {
        for c in nodes.into_iter().flatten() {
            let body = c.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if body.trim().is_empty() {
                continue;
            }
            out.push(RawPrComment {
                external_id: c.get("id").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                author: c.pointer("/author/login").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                body,
                created_at: c.get(date_field).and_then(|v| v.as_str()).unwrap_or("").to_string(),
                path: None,
                line: None,
            });
        }
    };
    push_simple(pr.pointer("/comments/nodes").and_then(|v| v.as_array()), "createdAt");
    push_simple(pr.pointer("/reviews/nodes").and_then(|v| v.as_array()), "submittedAt");

    for thread in pr.pointer("/reviewThreads/nodes").and_then(|v| v.as_array()).into_iter().flatten() {
        for c in thread.pointer("/comments/nodes").and_then(|v| v.as_array()).into_iter().flatten() {
            let body = c.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if body.trim().is_empty() {
                continue;
            }
            out.push(RawPrComment {
                // `databaseId`, not `id`: this is the source whose ids were
                // persisted as REST numerics.
                external_id: c.get("databaseId").and_then(|v| v.as_i64()).map(|n| n.to_string()).unwrap_or_default(),
                author: c.pointer("/author/login").and_then(|v| v.as_str()).unwrap_or("unknown").to_string(),
                body,
                created_at: c.get("createdAt").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                path: c.get("path").and_then(|v| v.as_str()).map(|s| s.to_string()),
                line: c.get("line").and_then(|v| v.as_i64()).or_else(|| c.get("originalLine").and_then(|v| v.as_i64())),
            });
        }
    }

    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    out
}

fn parse_graphql_pr(pr: &Value) -> Option<SnapshotPr> {
    Some(SnapshotPr {
        number: pr.get("number").and_then(|v| v.as_i64())?,
        url: pr.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        title: pr.get("title").and_then(|v| v.as_str()).map(|s| s.to_string()),
        created_at: pr.get("createdAt").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        updated_at: pr.get("updatedAt").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        state: pr.get("state").and_then(|v| v.as_str()).unwrap_or("OPEN").to_lowercase(),
        author_login: pr.pointer("/author/login").and_then(|v| v.as_str()).map(|s| s.to_string()),
        assignees: pr
            .pointer("/assignees/nodes")
            .and_then(|v| v.as_array())
            .map(|list| list.iter().filter_map(|a| a.get("login").and_then(|v| v.as_str()).map(|s| s.to_string())).collect())
            .unwrap_or_default(),
        head_branch: pr.get("headRefName").and_then(|v| v.as_str()).map(|s| s.to_string()),
        base_branch: pr.get("baseRefName").and_then(|v| v.as_str()).map(|s| s.to_string()),
        // Always `Ready`: one GitHub query carries every comment source, so
        // there is never anything left to go back for.
        comments: PendingComments::Ready(parse_graphql_pr_comments(pr)),
    })
}

/// Parses the repo-wide query's response. Pure, so the whole shape of a poll
/// pass is testable against a fixture instead of a live repo.
fn parse_repo_snapshot(json: &Value) -> RepoPrSnapshot {
    RepoPrSnapshot {
        viewer_login: json.pointer("/data/viewer/login").and_then(|v| v.as_str()).map(|s| s.to_string()),
        review_requested: json
            .pointer("/data/reviewRequested/nodes")
            .and_then(|v| v.as_array())
            .map(|rows| rows.iter().filter_map(|r| r.get("number").and_then(|v| v.as_i64())).collect())
            .unwrap_or_default(),
        prs: json
            .pointer("/data/repository/pullRequests/nodes")
            .and_then(|v| v.as_array())
            .map(|nodes| nodes.iter().filter_map(parse_graphql_pr).collect())
            .unwrap_or_default(),
    }
}

/// One round trip for one repo: the open PRs with their comments, who the
/// viewer is, and which PRs GitHub itself says are waiting on the viewer's
/// review.
///
/// This replaced a fan-out that cost, per pass: two `gh` calls per repo plus
/// one to three per tracked PR (a state probe, a comments fetch, and a REST
/// call for inline comments), each a process spawn of ~0.5-0.9s. Measured on
/// this machine: 11-21 processes and 10-20 seconds became one 0.4-0.8s call per
/// repo. `viewer { login }` also folds in what used to be a separate
/// `gh api user` per PR.
pub fn fetch_repo_snapshot(repo: &RepoRef, limit: usize) -> Result<RepoPrSnapshot, String> {
    let query = format!(
        r#"query($owner:String!, $name:String!, $limit:Int!, $search:String!) {{
  viewer {{ login }}
  repository(owner:$owner, name:$name) {{
    pullRequests(states: OPEN, first: $limit, orderBy: {{field: UPDATED_AT, direction: DESC}}) {{ nodes {{ ...prFields }} }}
  }}
  reviewRequested: search(query: $search, type: ISSUE, first: 100) {{ nodes {{ ... on PullRequest {{ number }} }} }}
}}{PR_FIELDS_FRAGMENT}"#
    );
    let search = format!("repo:{} is:pr is:open review-requested:@me", repo.path());
    let limit = limit.to_string();
    let output = gh_output(
        None,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("owner={}", repo.namespace),
            "-F",
            &format!("name={}", repo.name),
            "-F",
            &format!("limit={limit}"),
            "-f",
            &format!("search={search}"),
        ],
    )?;
    if !output.status.success() {
        return Err(format!("gh api graphql failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    Ok(parse_repo_snapshot(&json))
}

pub fn fetch_single_pr_facts(repo: &RepoRef, number: i64) -> Result<PrFacts, String> {
    let query = format!(
        r#"query($owner:String!, $name:String!, $number:Int!) {{
  viewer {{ login }}
  repository(owner:$owner, name:$name) {{ pullRequest(number:$number) {{ ...prFields }} }}
}}{PR_FIELDS_FRAGMENT}"#
    );
    let output = gh_output(
        None,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-F",
            &format!("owner={}", repo.namespace),
            "-F",
            &format!("name={}", repo.name),
            "-F",
            &format!("number={number}"),
        ],
    )?;
    if !output.status.success() {
        return Err(format!("gh api graphql failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let node = json.pointer("/data/repository/pullRequest").ok_or_else(|| "gh returned no pull request".to_string())?;
    let parsed = parse_graphql_pr(node).ok_or_else(|| "gh returned a pull request with no number".to_string())?;
    Ok(PrFacts {
        state: parsed.state,
        updated_at: parsed.updated_at,
        comments: parsed.comments,
        viewer_login: json.pointer("/data/viewer/login").and_then(|v| v.as_str()).map(|s| s.to_string()),
    })
}

/// Unreachable in practice — GitHub's snapshots and single-PR queries always
/// produce `PendingComments::Ready`, so nothing ever defers. Implemented
/// honestly rather than with a panic so the dispatch stays total.
pub fn fetch_comments(repo: &RepoRef, number: i64) -> Result<Vec<RawPrComment>, String> {
    fetch_single_pr_facts(repo, number)?.comments.resolve()
}

pub fn create_change_request(_repo: &RepoRef, cwd: &Path, req: &NewChangeRequest) -> Result<CreatedChangeRequest, String> {
    let output = gh_output(
        Some(cwd),
        &["pr", "create", "--title", req.title, "--body", req.body, "--base", req.base, "--head", req.head],
    )?;
    if !output.status.success() {
        return Err(format!("gh pr create failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let number: i64 = url.rsplit('/').next().and_then(|s| s.parse().ok()).ok_or_else(|| format!("could not parse a PR number out of: {url}"))?;
    Ok(CreatedChangeRequest { url, number })
}

pub fn fetch_head_branch(repo: &RepoRef, number: i64) -> Result<String, String> {
    let output = gh_output(None, &["pr", "view", &number.to_string(), "--repo", &repo.path(), "--json", "headRefName"])?;
    if !output.status.success() {
        return Err(format!("gh pr view failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    Ok(json.get("headRefName").and_then(|v| v.as_str()).ok_or_else(|| "gh did not return a headRefName".to_string())?.to_string())
}

pub fn default_branch(cwd: &Path) -> Option<String> {
    let output = gh_output(Some(cwd), &["repo", "view", "--json", "defaultBranchRef", "-q", ".defaultBranchRef.name"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

/// Who `gh` is signed in as on this instance.
pub fn viewer_login(host: &str) -> Option<String> {
    let output = gh_output(None, &["api", "user", "--hostname", host, "--jq", ".login"]).ok()?;
    if !output.status.success() {
        return None;
    }
    let login = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!login.is_empty()).then_some(login)
}

/// Hands `gh` a token on stdin, so it never reaches argv or a log.
///
/// `--with-token` reads exactly one line from stdin and is `gh`'s own affordance
/// for a scripted login.
pub fn login_with_token(host: &str, token: &str) -> Result<(), String> {
    let mut cmd = std::process::Command::new(providers::resolve_binary("gh"));
    cmd.args(["auth", "login", "--hostname", host, "--with-token"]);
    let output = crate::process_timeout::run_with_stdin(cmd, token, crate::process_timeout::DEFAULT_TIMEOUT)?;
    if !output.status.success() {
        return Err(format!("gh auth login failed on {host}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

/// The repositories this login can see, most recently pushed first.
///
/// `user/repos` rather than `search`: it needs no query, it includes private
/// repos and organisation ones the user is a member of, and it is the same
/// endpoint on github.com and on Enterprise.
pub fn list_repos(host: &str) -> Result<Vec<super::InstanceRepo>, String> {
    let output = gh_output(None, &["api", "user/repos?per_page=100&sort=pushed", "--hostname", host])?;
    if !output.status.success() {
        return Err(format!("gh api user/repos failed on {host}: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    let json: Value = serde_json::from_slice(&output.stdout).map_err(|e| format!("gh returned unparseable JSON: {e}"))?;
    Ok(json
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let path = r.get("full_name")?.as_str()?.to_string();
            Some(super::InstanceRepo {
                name: r.get("name").and_then(|v| v.as_str()).unwrap_or(&path).to_string(),
                path,
                description: r.get("description").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                web_url: r.get("html_url").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                clone_url: r.get("ssh_url").or_else(|| r.get("clone_url")).and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            })
        })
        .filter(|r| !r.clone_url.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The snapshot's comments are always `Ready` on GitHub; this unwraps them
    /// so the assertions below read the same as before `PendingComments`
    /// existed.
    fn ready(comments: &PendingComments) -> &[RawPrComment] {
        match comments {
            PendingComments::Ready(list) => list,
            PendingComments::Deferred { .. } => panic!("GitHub never defers its comments"),
        }
    }

    // -- the one-query-per-repo snapshot ------------------------------------

    /// A response in the exact shape the real query returns (verified against
    /// `cli/cli` before this was written): one PR with all three comment
    /// sources, an empty-bodied approval, and an inline comment whose `line`
    /// GitHub has nulled out.
    fn snapshot_json() -> Value {
        serde_json::from_str(
            r#"{
              "data": {
                "viewer": {"login": "killua15"},
                "repository": {"pullRequests": {"nodes": [
                  {
                    "number": 128, "url": "https://github.com/killua15/Hangar/pull/128",
                    "title": "Fix billing rounding", "createdAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-02T00:00:00Z", "state": "OPEN",
                    "author": {"login": "alice"},
                    "assignees": {"nodes": [{"login": "killua15"}]},
                    "headRefName": "feat/billing", "baseRefName": "main",
                    "comments": {"nodes": [
                      {"id": "IC_kwDOaaa", "body": "please fix the typo", "createdAt": "2026-01-01T00:00:00Z", "author": {"login": "alice"}}
                    ]},
                    "reviews": {"nodes": [
                      {"id": "PRR_kwDObbb", "body": "", "submittedAt": "2026-01-01T00:01:00Z", "author": {"login": "bob"}},
                      {"id": "PRR_kwDOccc", "body": "one more thing", "submittedAt": "2026-01-01T00:02:00Z", "author": {"login": "carol"}}
                    ]},
                    "reviewThreads": {"nodes": [
                      {"comments": {"nodes": [
                        {"databaseId": 4772643945, "body": "this lookup can return null", "createdAt": "2026-01-01T00:03:00Z", "path": "src/billing.rs", "line": 40, "originalLine": 40, "author": {"login": "dave"}},
                        {"databaseId": 4772643946, "body": "and a null check here", "createdAt": "2026-01-01T00:04:00Z", "path": "src/billing.rs", "line": null, "originalLine": 62, "author": {"login": "dave"}}
                      ]}}
                    ]}
                  }
                ]}},
                "reviewRequested": {"nodes": [{"number": 128}]}
              }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn the_snapshot_carries_the_viewer_the_review_requests_and_the_prs() {
        let snapshot = parse_repo_snapshot(&snapshot_json());
        assert_eq!(snapshot.viewer_login.as_deref(), Some("killua15"), "the viewer comes free in the same query — no separate `gh api user`");
        assert!(snapshot.review_requested.contains(&128));
        assert_eq!(snapshot.prs.len(), 1);

        let pr = &snapshot.prs[0];
        assert_eq!(pr.number, 128);
        assert_eq!(pr.state, "open", "GitHub says OPEN; the column stores lowercase");
        assert_eq!(pr.author_login.as_deref(), Some("alice"));
        assert_eq!(pr.assignees, vec!["killua15".to_string()]);
        assert_eq!(pr.base_branch.as_deref(), Some("main"));
    }

    /// The three comment sources GitHub keeps apart arrive merged, oldest
    /// first, with bodyless reviews dropped — the same contract the two REST
    /// parsers used to provide between them.
    #[test]
    fn one_query_returns_all_three_comment_sources_merged_and_ordered() {
        let snapshot = parse_repo_snapshot(&snapshot_json());
        let comments = ready(&snapshot.prs[0].comments);

        assert_eq!(comments.len(), 4, "1 comment + 1 review with a body + 2 thread comments; the empty approval is dropped");
        let authors: Vec<&str> = comments.iter().map(|c| c.author.as_str()).collect();
        assert_eq!(authors, vec!["alice", "carol", "dave", "dave"], "oldest first");

        assert_eq!(comments[2].path.as_deref(), Some("src/billing.rs"));
        assert_eq!(comments[2].line, Some(40));
        assert_eq!(comments[3].line, Some(62), "must fall back to originalLine once GitHub nulls out line");
    }

    /// **The migration hazard.** `external_id` is how dedup works, and rows
    /// already in the database were written from `gh pr view --json
    /// comments,reviews` (GraphQL node ids) and from the REST inline endpoint
    /// (numeric ids). If GraphQL's field choice produced different strings,
    /// every comment ever seen would look new and the inbox would flood on the
    /// first pass. Verified against real API responses: `gh pr view`'s
    /// `reviews[].id` is byte-identical to GraphQL's `id`.
    #[test]
    fn comment_ids_keep_the_shape_already_persisted_so_nothing_looks_new() {
        let snapshot = parse_repo_snapshot(&snapshot_json());
        let ids: Vec<&str> = ready(&snapshot.prs[0].comments).iter().map(|c| c.external_id.as_str()).collect();

        assert_eq!(ids[0], "IC_kwDOaaa", "issue comments keep the node id");
        assert_eq!(ids[1], "PRR_kwDOccc", "reviews keep the node id");
        assert_eq!(ids[2], "4772643945", "inline comments keep the REST numeric id, via databaseId");
        assert_eq!(ids[3], "4772643946");
    }

    /// A repo with nothing open must parse to an empty snapshot rather than
    /// failing — that's the steady state of most projects.
    #[test]
    fn an_empty_repo_parses_to_an_empty_snapshot() {
        let json: Value =
            serde_json::from_str(r#"{"data":{"viewer":{"login":"killua15"},"repository":{"pullRequests":{"nodes":[]}},"reviewRequested":{"nodes":[]}}}"#).unwrap();
        let snapshot = parse_repo_snapshot(&json);
        assert!(snapshot.prs.is_empty());
        assert!(snapshot.review_requested.is_empty());
        assert_eq!(snapshot.viewer_login.as_deref(), Some("killua15"));
    }
}
