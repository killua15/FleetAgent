//! ClickUp tasks, over its REST API with a personal token.
//!
//! This is the first outgoing HTTP request Hangar makes from Rust — everything
//! before it was SQLite, the keychain and child processes. `reqwest` comes in
//! with `rustls` rather than the system TLS stack, so the build doesn't depend
//! on whatever OpenSSL happens to be on the machine.
//!
//! A personal token (`pk_…`) is deliberately all this asks for: it needs no app
//! registration, no OAuth callback and no approval, and it authenticates as
//! *you*, which is exactly the scope of "show me my tasks". One token covers
//! every workspace you belong to, because a ClickUp token belongs to a user
//! rather than to a workspace.

use super::{Task, TaskSourceLink};
use std::time::Duration;

const API: &str = "https://api.clickup.com/api/v2";

/// Long enough for a slow board, short enough that a hung request doesn't hold
/// a blocking thread while the user stares at a spinner.
const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Whoami {
    pub user_id: i64,
    pub username: String,
    pub team_ids: Vec<String>,
}

fn get(url: &str, token: &str) -> Result<serde_json::Value, String> {
    let client = reqwest::blocking::Client::builder().timeout(TIMEOUT).build().map_err(|e| format!("could not start an HTTP client: {e}"))?;
    let response = client.get(url).header("Authorization", token).send().map_err(|e| {
        if e.is_timeout() {
            "ClickUp didn't answer in time".to_string()
        } else if e.is_connect() {
            "couldn't reach ClickUp — check the network".to_string()
        } else {
            format!("request to ClickUp failed: {e}")
        }
    })?;

    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        // ClickUp answers 401 for a bad token and 200-with-nothing for a token
        // that's fine but sees no tasks. Distinguishing them here is what keeps
        // "your token expired" from looking like "you have no work".
        return Err(match status.as_u16() {
            401 => "ClickUp rejected the token — regenerate it in Settings → Apps → API Token".to_string(),
            403 => "that token can't see this workspace".to_string(),
            429 => "ClickUp rate-limited the request — try again in a minute".to_string(),
            code => format!("ClickUp answered {code}: {}", body.trim()),
        });
    }
    serde_json::from_str(&body).map_err(|e| format!("could not read ClickUp's answer: {e}"))
}

/// Confirms a token works and collects what the fetch will need, so a wrong
/// token fails at the form where it can be fixed rather than at the panel.
pub fn verify(token: &str) -> Result<Whoami, String> {
    let me = get(&format!("{API}/user"), token)?;
    let user = me.get("user").ok_or("ClickUp didn't return a user for that token")?;
    let user_id = user.get("id").and_then(|id| id.as_i64()).ok_or("ClickUp returned a user with no id")?;
    let username = user.get("username").and_then(|u| u.as_str()).unwrap_or("").to_string();

    let teams = get(&format!("{API}/team"), token)?;
    let team_ids: Vec<String> = teams
        .get("teams")
        .and_then(|t| t.as_array())
        .map(|teams| teams.iter().filter_map(|team| team.get("id").and_then(value_as_id)).collect())
        .unwrap_or_default();
    if team_ids.is_empty() {
        return Err("that token works but belongs to no workspace".to_string());
    }

    Ok(Whoami { user_id, username, team_ids })
}

/// ClickUp is inconsistent about ids: team and list ids come back as strings,
/// user ids as numbers, and task ids as strings. Reading either shape here is
/// cheaper than being surprised by it per call site.
fn value_as_id(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string).or_else(|| value.as_i64().map(|n| n.to_string()))
}

/// ClickUp's page size on the filtered endpoints. A short page means the last
/// one, which is how the loop knows to stop.
const PAGE: usize = 100;

/// How many pages to walk before giving up. Five is 500 tasks — past that, the
/// answer isn't "paginate harder", it's "scope this project to a list".
const MAX_PAGES: usize = 5;

pub fn fetch(link: &TaskSourceLink, token: &str, config: &serde_json::Value) -> Result<Vec<Task>, String> {
    let user_id = config.get("userId").and_then(|id| id.as_i64()).ok_or("this source needs re-saving — its ClickUp user is unknown")?;

    // A link scoped to one list is the precise case: "this repo is that list".
    // Without a scope it falls back to the whole workspace, which is the right
    // default for one person with one board.
    let bases: Vec<String> = match link.scope_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(list_id) => vec![format!("{API}/list/{list_id}/task")],
        None => {
            let teams: Vec<String> = config
                .get("teamIds")
                .and_then(|t| t.as_array())
                .map(|ids| ids.iter().filter_map(value_as_id).collect())
                .unwrap_or_default();
            if teams.is_empty() {
                return Err("this source needs re-saving — its ClickUp workspaces are unknown".to_string());
            }
            teams.iter().map(|team| format!("{API}/team/{team}/task")).collect()
        }
    };

    let mut tasks = Vec::new();
    for base in &bases {
        // Ask the server to filter first: it's one request and it's usually
        // right. `assignees[]` behaves differently across ClickUp's endpoints
        // and plan tiers, though, and when it quietly returns nothing there is
        // no way to tell that from "you have no tasks" — so an empty answer
        // falls through to reading the pages and matching on the same
        // `assignees` array the board itself shows.
        let filtered = fetch_pages(base, token, Some(user_id), &link.source_id)?;
        if !filtered.is_empty() {
            tasks.extend(filtered.into_iter().map(|(task, _)| task));
            continue;
        }
        let scanned = fetch_pages(base, token, None, &link.source_id)?;
        tasks.extend(scanned.into_iter().filter(|(_, assignees)| assignees.contains(&user_id)).map(|(task, _)| task));
    }
    Ok(tasks)
}

/// One thing a project can be pointed at. Only lists: a space or a folder is a
/// container, and pointing a repo at a container would mean deciding what to do
/// when someone adds a list to it later.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub id: String,
    /// `Space / Folder / List`, because "Tasks" is a list name in four places
    /// and the id underneath it is a number nobody can tell apart.
    pub path: String,
}

/// Every list the token can see, so the settings screen can offer a menu
/// instead of asking for an id off a ClickUp URL.
pub fn list_scopes(token: &str, config: &serde_json::Value) -> Result<Vec<Scope>, String> {
    let teams: Vec<String> = config
        .get("teamIds")
        .and_then(|t| t.as_array())
        .map(|ids| ids.iter().filter_map(value_as_id).collect())
        .unwrap_or_default();

    let mut scopes = Vec::new();
    for team in teams {
        let spaces = get(&format!("{API}/team/{team}/space?archived=false"), token)?;
        for space in spaces.get("spaces").and_then(|s| s.as_array()).cloned().unwrap_or_default() {
            let Some(space_id) = space.get("id").and_then(value_as_id) else { continue };
            let space_name = space.get("name").and_then(|n| n.as_str()).unwrap_or("space").to_string();

            // Lists that sit straight in the space, with no folder around them.
            if let Ok(loose) = get(&format!("{API}/space/{space_id}/list?archived=false"), token) {
                collect_lists(&loose, &space_name, None, &mut scopes);
            }

            let folders = get(&format!("{API}/space/{space_id}/folder?archived=false"), token)?;
            for folder in folders.get("folders").and_then(|f| f.as_array()).cloned().unwrap_or_default() {
                let folder_name = folder.get("name").and_then(|n| n.as_str()).unwrap_or("folder").to_string();
                // The folder response already embeds its lists — asking per
                // folder would be one request each for nothing.
                collect_lists(&folder, &space_name, Some(&folder_name), &mut scopes);
            }
        }
    }
    Ok(scopes)
}

fn collect_lists(container: &serde_json::Value, space: &str, folder: Option<&str>, out: &mut Vec<Scope>) {
    for list in container.get("lists").and_then(|l| l.as_array()).cloned().unwrap_or_default() {
        let Some(id) = list.get("id").and_then(value_as_id) else { continue };
        let name = list.get("name").and_then(|n| n.as_str()).unwrap_or("list");
        let path = match folder {
            Some(folder) => format!("{space} / {folder} / {name}"),
            None => format!("{space} / {name}"),
        };
        out.push(Scope { id, path });
    }
}

/// Walks the pages of one endpoint. With `assignee` the filter is ClickUp's;
/// without it, the caller does the matching from the returned assignee ids.
fn fetch_pages(base: &str, token: &str, assignee: Option<i64>, source_id: &str) -> Result<Vec<(Task, Vec<i64>)>, String> {
    let mut out = Vec::new();
    for page in 0..MAX_PAGES {
        let filter = assignee.map(|id| format!("&assignees%5B%5D={id}")).unwrap_or_default();
        let url = format!("{base}?include_closed=false&subtasks=true&page={page}{filter}");
        let body = get(&url, token)?;
        let batch = parse_tasks_with_assignees(&body, source_id);
        let full_page = batch.len() >= PAGE;
        out.extend(batch);
        // ClickUp also sends `last_page`, but only on some endpoints — a short
        // page is the signal that works on all of them.
        if !full_page || body.get("last_page").and_then(|l| l.as_bool()).unwrap_or(false) {
            break;
        }
    }
    Ok(out)
}

/// The key and the title a task should show, given its raw name.
///
/// Teams that don't pay for ClickUp's custom ids put the key in the title
/// instead — `[TMS-task-09] Signal catalog` — and it's the name everyone
/// actually says. Without this the branch would be
/// `CU-86ajhva8z-tms-task-09-signal-catalog`: the hash nobody recognises,
/// followed by the key repeated inside the slug.
///
/// The prefix is *removed* from the title on purpose: it's shown as the key
/// beside it, and leaving it in both places would put it in the branch twice.
pub(crate) fn key_from_title(name: &str) -> Option<(String, String)> {
    let trimmed = name.trim_start();
    let inner = trimmed.strip_prefix('[')?.split_once(']')?;
    let key = inner.0.trim();
    // What separates a key from a label is a number: `TMS-task-09` and `DEV-42`
    // identify one task, `[URGENTE]` and `[BUG]` classify many. Taking a label
    // as the key would give every urgent task the same branch name.
    if key.is_empty() || key.len() > 24 || key.contains(' ') || !key.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    Some((key.to_string(), inner.1.trim().to_string()))
}

/// Split from `fetch` so the shape ClickUp returns is testable without a token
/// or a network.
pub(crate) fn parse_tasks(body: &serde_json::Value, source_id: &str) -> Vec<Task> {
    parse_tasks_with_assignees(body, source_id).into_iter().map(|(task, _)| task).collect()
}

/// The same, keeping each task's assignee ids so the caller can filter when
/// ClickUp's own `assignees[]` filter came back empty.
pub(crate) fn parse_tasks_with_assignees(body: &serde_json::Value, source_id: &str) -> Vec<(Task, Vec<i64>)> {
    let Some(list) = body.get("tasks").and_then(|t| t.as_array()) else { return Vec::new() };
    list.iter()
        .filter_map(|task| {
            let id = task.get("id").and_then(value_as_id)?;
            let name = task.get("name").and_then(|n| n.as_str()).unwrap_or("untitled");
            let from_title = key_from_title(name);
            // `custom_id` is the human key a team actually says out loud
            // ("DEV-42"); the raw id is a hash nobody recognises. Prefer it,
            // then a key written into the title, then the id.
            let custom = task.get("custom_id").and_then(|c| c.as_str()).filter(|c| !c.is_empty());
            let assignees: Vec<i64> = task
                .get("assignees")
                .and_then(|a| a.as_array())
                .map(|list| list.iter().filter_map(|a| a.get("id").and_then(|id| id.as_i64())).collect())
                .unwrap_or_default();
            Some((Task {
                key: custom
                    .map(str::to_string)
                    .or_else(|| from_title.as_ref().map(|(key, _)| key.clone()))
                    .unwrap_or_else(|| format!("CU-{id}")),
                external_id: id,
                title: from_title.map(|(_, title)| title).unwrap_or_else(|| name.to_string()),
                // `text_content` is the description as plain text; `description`
                // is the same thing with ClickUp's markup. The agent gets more
                // out of the plain one.
                description: task
                    .get("text_content")
                    .and_then(|d| d.as_str())
                    .or_else(|| task.get("description").and_then(|d| d.as_str()))
                    .unwrap_or("")
                    .to_string(),
                url: task.get("url").and_then(|u| u.as_str()).unwrap_or("").to_string(),
                status: task.get("status").and_then(|s| s.get("status")).and_then(|s| s.as_str()).map(str::to_string),
                branch_hint: None,
                source_id: source_id.to_string(),
                source_kind: "clickup".to_string(),
                suggested_branch: String::new(),
            }, assignees))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).unwrap()
    }

    #[test]
    fn tasks_become_tasks() {
        let parsed = parse_tasks(
            &body(
                r#"{"tasks": [{
                  "id": "86abc12", "custom_id": null, "name": "Signal catalog seed",
                  "text_content": "Seed the catalog.", "url": "https://app.clickup.com/t/86abc12",
                  "status": {"status": "in progress"}
                }]}"#,
            ),
            "src1",
        );
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].external_id, "86abc12");
        assert_eq!(parsed[0].key, "CU-86abc12", "without a custom id the raw id carries the key");
        assert_eq!(parsed[0].status.as_deref(), Some("in progress"));
    }

    /// A team using ClickUp's custom ids says "DEV-42", never the hash — and
    /// the branch name is the thing everyone else on the team has to recognise.
    #[test]
    fn a_custom_id_becomes_the_key() {
        let parsed = parse_tasks(
            &body(r#"{"tasks": [{"id": "86abc12", "custom_id": "DEV-42", "name": "x", "url": "u", "status": {"status": "open"}}]}"#),
            "src1",
        );
        assert_eq!(parsed[0].key, "DEV-42");
        assert_eq!(parsed[0].external_id, "86abc12", "the id stays the identity even when the key is prettier");
    }

    /// An empty board and a malformed answer must both come back as no tasks
    /// rather than panicking inside a blocking thread.
    #[test]
    fn an_empty_or_unexpected_answer_yields_nothing() {
        assert!(parse_tasks(&body(r#"{"tasks": []}"#), "src1").is_empty());
        assert!(parse_tasks(&body(r#"{"err": "oops"}"#), "src1").is_empty());
    }

    /// Real boards that don't pay for custom ids put the key in the title.
    /// Taken from an actual workspace: `[TMS-task-09] Signal catalog (seed +
    /// endpoint)` must branch as `TMS-task-09-signal-catalog…`, not as
    /// `CU-86ajhva8z-tms-task-09-signal-catalog…`.
    #[test]
    fn a_key_written_into_the_title_wins_over_the_hash() {
        let parsed = parse_tasks(
            &body(r#"{"tasks": [{"id": "86ajhva8z", "custom_id": null, "name": "[TMS-task-09] Signal catalog (seed + endpoint)", "url": "u", "status": {"status": "verificar"}}]}"#),
            "src1",
        );
        assert_eq!(parsed[0].key, "TMS-task-09");
        assert_eq!(parsed[0].title, "Signal catalog (seed + endpoint)", "the prefix moves to the key rather than being repeated in the branch");
    }

    /// A bracket is only a key when it looks like one. A shout or a sentence in
    /// brackets is part of the title.
    #[test]
    fn a_bracket_that_is_not_a_key_is_left_alone() {
        assert_eq!(key_from_title("[URGENTE] revisar o fluxo todo"), None, "a label classifies many tasks — it can't name one branch");
        assert_eq!(key_from_title("[refatorar tudo] x"), None, "spaces mean it's prose");
        assert_eq!(key_from_title("[] nada"), None);
        assert_eq!(key_from_title("no brackets here"), None);
        assert_eq!(key_from_title("[DEV-42] fix it").unwrap().0, "DEV-42");
    }

    /// The fallback path when ClickUp's own `assignees[]` filter returns
    /// nothing: the ids come back with each task so the caller can match on
    /// exactly what the board shows.
    #[test]
    fn assignee_ids_come_back_with_each_task() {
        let parsed = parse_tasks_with_assignees(
            &body(r#"{"tasks": [{"id": "1", "name": "mine", "url": "u", "assignees": [{"id": 82117818, "username": "David"}]},
                                {"id": "2", "name": "someone else's", "url": "u", "assignees": [{"id": 3008255}]},
                                {"id": "3", "name": "nobody's", "url": "u", "assignees": []}]}"#),
            "src1",
        );
        let mine: Vec<_> = parsed.iter().filter(|(_, a)| a.contains(&82117818)).map(|(t, _)| t.title.as_str()).collect();
        assert_eq!(mine, vec!["mine"]);
    }

    /// ClickUp returns ids as strings in some endpoints and numbers in others.
    #[test]
    fn ids_are_read_in_either_shape() {
        assert_eq!(value_as_id(&serde_json::json!("123")), Some("123".to_string()));
        assert_eq!(value_as_id(&serde_json::json!(123)), Some("123".to_string()));
    }
}
