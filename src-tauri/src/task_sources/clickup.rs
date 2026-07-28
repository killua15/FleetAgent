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

pub fn fetch(link: &TaskSourceLink, token: &str, config: &serde_json::Value) -> Result<Vec<Task>, String> {
    let user_id = config.get("userId").and_then(|id| id.as_i64()).ok_or("this source needs re-saving — its ClickUp user is unknown")?;

    // A link scoped to one list is the precise case: "this repo is that list".
    // Without a scope it falls back to everything assigned to you across the
    // workspace, which is the right default for a solo user with one board.
    let urls: Vec<String> = match link.scope_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(list_id) => vec![format!("{API}/list/{list_id}/task?assignees%5B%5D={user_id}&include_closed=false&subtasks=true")],
        None => {
            let teams: Vec<String> = config
                .get("teamIds")
                .and_then(|t| t.as_array())
                .map(|ids| ids.iter().filter_map(value_as_id).collect())
                .unwrap_or_default();
            if teams.is_empty() {
                return Err("this source needs re-saving — its ClickUp workspaces are unknown".to_string());
            }
            teams.iter().map(|team| format!("{API}/team/{team}/task?assignees%5B%5D={user_id}&include_closed=false&subtasks=true")).collect()
        }
    };

    let mut tasks = Vec::new();
    for url in urls {
        let body = get(&url, token)?;
        tasks.extend(parse_tasks(&body, &link.source_id));
    }
    Ok(tasks)
}

/// Split from `fetch` so the shape ClickUp returns is testable without a token
/// or a network.
pub(crate) fn parse_tasks(body: &serde_json::Value, source_id: &str) -> Vec<Task> {
    let Some(list) = body.get("tasks").and_then(|t| t.as_array()) else { return Vec::new() };
    list.iter()
        .filter_map(|task| {
            let id = task.get("id").and_then(value_as_id)?;
            // `custom_id` is the human key a team actually says out loud
            // ("DEV-42"); the raw id is a hash nobody recognises. Prefer it for
            // the branch name and fall back to the id.
            let custom = task.get("custom_id").and_then(|c| c.as_str()).filter(|c| !c.is_empty());
            Some(Task {
                key: custom.map(str::to_string).unwrap_or_else(|| format!("CU-{id}")),
                external_id: id,
                title: task.get("name").and_then(|n| n.as_str()).unwrap_or("untitled").to_string(),
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
            })
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

    /// ClickUp returns ids as strings in some endpoints and numbers in others.
    #[test]
    fn ids_are_read_in_either_shape() {
        assert_eq!(value_as_id(&serde_json::json!("123")), Some("123".to_string()));
        assert_eq!(value_as_id(&serde_json::json!(123)), Some("123".to_string()));
    }
}
