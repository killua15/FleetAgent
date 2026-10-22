//! Any knowledge tool that speaks MCP over HTTP — Notion's, Confluence's, a
//! BookStack bridge — reached by URL and, when it wants one, a bearer token.
//!
//! The door for "my team uses something else": Hangar knows nothing about the
//! tool beyond its MCP endpoint, so all it can do is hand that endpoint to the
//! agents. That is the main thing anyway.

use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

pub fn normalize_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("the MCP address has to start with https:// — e.g. https://mcp.acme.com/mcp".to_string());
    }
    Ok(trimmed.to_string())
}

/// Sends `initialize` and reads the server's name back — proof that the URL is
/// an MCP server and that it takes the token, before anything is stored.
pub fn verify(url: &str, token: Option<&str>) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder().timeout(TIMEOUT).build().map_err(|e| format!("could not start an HTTP client: {e}"))?;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "hangar", "version": env!("CARGO_PKG_VERSION") } }
    });
    let mut request = client.post(url).header("Accept", "application/json, text/event-stream").json(&body);
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    let response = request.send().map_err(|e| {
        if e.is_timeout() {
            "the MCP server didn't answer in time".to_string()
        } else if e.is_connect() {
            format!("couldn't reach {url} — check the address and the network")
        } else {
            format!("request to the MCP server failed: {e}")
        }
    })?;
    let status = response.status();
    let text = response.text().unwrap_or_default();
    match status.as_u16() {
        200..=299 => Ok(server_name(&text).unwrap_or_else(|| "MCP server".to_string())),
        401 | 403 => Err(if token.is_some() {
            "the MCP server refused that token".to_string()
        } else {
            "the MCP server wants a token — paste one".to_string()
        }),
        404 | 405 => Err(format!("{url} doesn't answer MCP requests — is this the server's MCP endpoint?")),
        code => Err(format!("the MCP server answered {code}: {}", text.trim())),
    }
}

/// The `serverInfo.name` out of an `initialize` answer, which comes back as
/// plain JSON or as a one-event SSE stream depending on the server.
fn server_name(body: &str) -> Option<String> {
    let json = serde_json::from_str::<serde_json::Value>(body.trim()).ok().or_else(|| {
        body.lines().filter_map(|l| l.strip_prefix("data:")).find_map(|data| serde_json::from_str::<serde_json::Value>(data.trim()).ok())
    })?;
    json.pointer("/result/serverInfo/name").and_then(|v| v.as_str()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_name_is_read_from_json_or_from_an_event_stream() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"outline","version":"1"}}}"#;
        assert_eq!(server_name(json).as_deref(), Some("outline"));
        let sse = format!("event: message\ndata: {json}\n\n");
        assert_eq!(server_name(&sse).as_deref(), Some("outline"));
        assert_eq!(server_name("not json"), None);
    }
}
