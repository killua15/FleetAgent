//! The MCP servers Hangar hands an agent: the code graph (graphify) and the
//! team's knowledge sources (`crate::knowledge_sources`), in the shape each CLI
//! takes them.
//!
//! **Tokens never touch the command line or the config file.** A terminal's
//! command is typed into a shell that records it in `~/.zsh_history` and its
//! scrollback, so — like an account's API key (`terminals::account_key_env`) —
//! a wiki's token travels as process environment, and the config only names the
//! variable. Verified on Claude Code 2.1.308 (`${VAR}` in an `--mcp-config`
//! header is expanded) and Codex 0.147.0 (`bearer_token_env_var`): both sent
//! `Authorization: Bearer <token>` read from the environment.
//!
//! Additive, as graphify always was: `--mcp-config` and never
//! `--strict-mcp-config`, so the user's own servers stay on.

use crate::knowledge_sources::AgentServer;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Runtime};

/// A remote server, for a CLI configured key by key rather than by file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RemoteServer {
    pub name: String,
    pub url: String,
    /// The variable carrying its token, when it has one.
    pub token_env: Option<String>,
}

/// Everything an agent of this project gets.
#[derive(Debug, Clone, Default)]
pub(crate) struct AgentMcp {
    /// The `--mcp-config` file, for the CLIs that take one. `None` when there
    /// is nothing to serve — the common case — which means no flag at all.
    pub config: Option<PathBuf>,
    /// The same remote servers, for Codex's `-c mcp_servers.*` overrides.
    pub remote: Vec<RemoteServer>,
    /// Their tokens, to put in the process environment.
    pub env: Vec<(String, String)>,
}

/// `team-docs` → `HANGAR_MCP_TEAM_DOCS_TOKEN`.
pub(crate) fn token_env_name(server: &str) -> String {
    let upper: String = server.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }).collect();
    format!("HANGAR_MCP_{upper}_TOKEN")
}

/// The remote servers and their environment, from the sources a project gets.
pub(crate) fn remote_servers(sources: &[AgentServer]) -> (Vec<RemoteServer>, Vec<(String, String)>) {
    let mut env = Vec::new();
    let remote = sources
        .iter()
        .map(|s| {
            let token_env = s.token.as_ref().map(|token| {
                let var = token_env_name(&s.name);
                env.push((var.clone(), token.clone()));
                var
            });
            RemoteServer { name: s.name.clone(), url: s.url.clone(), token_env }
        })
        .collect();
    (remote, env)
}

/// The `--mcp-config` JSON: graphify (when built) and each remote server as an
/// `http` entry whose header names its variable instead of holding the token.
/// `None` when there is nothing in it.
pub(crate) fn config_json(graphify: Option<(&str, &Path)>, remote: &[RemoteServer]) -> Option<String> {
    if graphify.is_none() && remote.is_empty() {
        return None;
    }
    let mut servers = serde_json::Map::new();
    if let Some((binary, graph)) = graphify {
        servers.insert("graphify".to_string(), crate::graph::graphify_entry(binary, graph));
    }
    for server in remote {
        let mut entry = serde_json::json!({ "type": "http", "url": server.url });
        if let Some(var) = &server.token_env {
            entry["headers"] = serde_json::json!({ "Authorization": format!("Bearer ${{{var}}}") });
        }
        servers.insert(server.name.clone(), entry);
    }
    serde_json::to_string_pretty(&serde_json::json!({ "mcpServers": servers })).ok()
}

/// Codex's overrides for the remote servers. One `-c` per key, each a TOML
/// value — strings quoted — so `~/.codex/config.toml` is left untouched.
pub(crate) fn codex_args(remote: &[RemoteServer]) -> Vec<String> {
    let mut args = Vec::new();
    for server in remote {
        args.push("-c".to_string());
        args.push(format!("mcp_servers.{}.url={}", server.name, toml_string(&server.url)));
        if let Some(var) = &server.token_env {
            args.push("-c".to_string());
            args.push(format!("mcp_servers.{}.bearer_token_env_var={}", server.name, toml_string(var)));
        }
    }
    args
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// What this project's agents get, with the config written to
/// `<app data>/agent-mcp/<project>.json`. Best effort: a failure to write is a
/// log line and an agent without the extra servers, never a terminal that
/// won't open.
pub(crate) fn for_project<R: Runtime>(app: &AppHandle<R>, conn: &Connection, project_id: &str) -> AgentMcp {
    let graphify = crate::commands::graph::graphify_for_project(app, conn, project_id);
    let sources = crate::knowledge_sources::servers_for_project(conn, project_id);
    let (remote, env) = remote_servers(&sources);
    let Some(json) = config_json(graphify.as_ref().map(|(b, g)| (b.as_str(), g.as_path())), &remote) else {
        return AgentMcp::default();
    };
    let config = crate::app_data::root(app).ok().and_then(|root| {
        let dir = root.join("agent-mcp");
        let file = dir.join(format!("{project_id}.json"));
        match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&file, json)) {
            Ok(()) => Some(file),
            Err(e) => {
                log::warn!("agent_mcp: could not write {} — {e}", file.display());
                None
            }
        }
    });
    AgentMcp { config, remote, env }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wiki(token: Option<&str>) -> AgentServer {
        AgentServer { name: "team-docs".into(), url: "https://docs.example.com/mcp".into(), token: token.map(str::to_string) }
    }

    /// A project with only a graph gets exactly the file it always got.
    #[test]
    fn graphify_alone_is_the_same_config_as_before() {
        let graph = Path::new("/appdata/graphs/project-1/graphify-out/graph.json");
        let json = config_json(Some(("/bin/graphify-mcp", graph)), &[]).unwrap();
        assert_eq!(json, crate::graph::mcp_config_json("/bin/graphify-mcp", graph));
        assert_eq!(config_json(None, &[]), None, "nothing to serve is no file and no flag");
    }

    /// The token reaches the process environment and nowhere else.
    #[test]
    fn a_wiki_is_an_http_server_whose_token_stays_out_of_the_file() {
        let (remote, env) = remote_servers(&[wiki(Some("ol_api_secret"))]);
        assert_eq!(env, vec![("HANGAR_MCP_TEAM_DOCS_TOKEN".to_string(), "ol_api_secret".to_string())]);

        let json = config_json(None, &remote).unwrap();
        assert!(!json.contains("ol_api_secret"), "{json}");
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entry = &parsed["mcpServers"]["team-docs"];
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["url"], "https://docs.example.com/mcp");
        assert_eq!(entry["headers"]["Authorization"], "Bearer ${HANGAR_MCP_TEAM_DOCS_TOKEN}");
    }

    #[test]
    fn a_server_without_a_token_sends_no_header() {
        let (remote, env) = remote_servers(&[wiki(None)]);
        assert!(env.is_empty());
        let parsed: serde_json::Value = serde_json::from_str(&config_json(None, &remote).unwrap()).unwrap();
        assert!(parsed["mcpServers"]["team-docs"].get("headers").is_none());
        assert_eq!(codex_args(&remote), vec!["-c", "mcp_servers.team-docs.url=\"https://docs.example.com/mcp\""]);
    }

    #[test]
    fn codex_gets_the_url_and_the_variable_never_the_token() {
        let (remote, _) = remote_servers(&[wiki(Some("ol_api_secret"))]);
        let args = codex_args(&remote);
        assert_eq!(
            args,
            vec![
                "-c",
                "mcp_servers.team-docs.url=\"https://docs.example.com/mcp\"",
                "-c",
                "mcp_servers.team-docs.bearer_token_env_var=\"HANGAR_MCP_TEAM_DOCS_TOKEN\"",
            ]
        );
        assert!(!args.join(" ").contains("ol_api_secret"));
    }
}
