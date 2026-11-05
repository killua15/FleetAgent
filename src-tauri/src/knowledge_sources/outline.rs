//! Outline (getoutline.com, or any self-hosted instance), over its REST API with
//! a personal API key.
//!
//! The same key also opens the instance's own MCP server at `<base>/mcp` —
//! Outline's MCP route accepts API keys alongside OAuth — which is what lets one
//! secret serve both Hangar and every agent, whatever CLI it runs.

use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Whoami {
    pub user: String,
    pub team: String,
}

/// `https://docs.acme.com/`, `https://docs.acme.com/mcp` or `…/api` all mean
/// the same instance: what people paste is whatever URL was in front of them.
pub fn normalize_base(url: &str) -> Result<String, String> {
    let trimmed = url.trim().trim_end_matches('/');
    if !(trimmed.starts_with("https://") || trimmed.starts_with("http://")) {
        return Err("the Outline address has to start with https:// — e.g. https://docs.acme.com".to_string());
    }
    let base = trimmed.trim_end_matches("/mcp").trim_end_matches("/api").trim_end_matches('/');
    if base.split("://").nth(1).is_none_or(|host| host.is_empty()) {
        return Err("that address has no host in it".to_string());
    }
    Ok(base.to_string())
}

/// Where the instance's MCP server lives.
pub fn mcp_url(base: &str) -> String {
    format!("{}/mcp", base.trim_end_matches('/'))
}

fn post(base: &str, method: &str, token: &str, body: serde_json::Value) -> Result<serde_json::Value, String> {
    let client = reqwest::blocking::Client::builder().timeout(TIMEOUT).build().map_err(|e| format!("could not start an HTTP client: {e}"))?;
    let response = client.post(format!("{base}/api/{method}")).bearer_auth(token).json(&body).send().map_err(|e| {
        if e.is_timeout() {
            "Outline didn't answer in time".to_string()
        } else if e.is_connect() {
            format!("couldn't reach {base} — check the address and the network (VPN?)")
        } else {
            format!("request to Outline failed: {e}")
        }
    })?;

    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => "Outline rejected the API key — create one in Outline → Settings → API & Apps".to_string(),
            403 => "that API key isn't allowed to do this — give it read and write access".to_string(),
            404 => format!("{base} doesn't look like an Outline instance (no /api/{method})"),
            429 => "Outline rate-limited the request — try again in a minute".to_string(),
            code => format!("Outline answered {code}: {}", text.trim()),
        });
    }
    serde_json::from_str(&text).map_err(|e| format!("could not read Outline's answer: {e}"))
}

/// Confirms the key works, and says whose it is — so the settings row can read
/// "you on Acme" rather than "a token".
pub fn verify(base: &str, token: &str) -> Result<Whoami, String> {
    parse_auth_info(&post(base, "auth.info", token, serde_json::json!({}))?)
}

fn parse_auth_info(json: &serde_json::Value) -> Result<Whoami, String> {
    let user = json.pointer("/data/user/name").and_then(|v| v.as_str());
    let team = json.pointer("/data/team/name").and_then(|v| v.as_str());
    match (user, team) {
        (Some(user), Some(team)) => Ok(Whoami { user: user.to_string(), team: team.to_string() }),
        _ => Err("Outline answered, but not with a user — is this the right address?".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whatever_url_was_pasted_names_the_same_instance() {
        for pasted in ["https://docs.acme.com", "https://docs.acme.com/", "https://docs.acme.com/mcp", " https://docs.acme.com/api/ "] {
            assert_eq!(normalize_base(pasted).unwrap(), "https://docs.acme.com", "from {pasted:?}");
        }
        assert_eq!(mcp_url("https://docs.acme.com"), "https://docs.acme.com/mcp");
        assert!(normalize_base("docs.acme.com").is_err(), "no scheme is a typo, not a guess");
        assert!(normalize_base("https://").is_err());
    }

    /// `auth.info` as Outline returns it (trimmed).
    #[test]
    fn auth_info_names_the_user_and_the_workspace() {
        let json = serde_json::json!({
            "data": { "user": { "id": "u1", "name": "David" }, "team": { "id": "t1", "name": "Acme" } },
            "ok": true
        });
        let who = parse_auth_info(&json).unwrap();
        assert_eq!((who.user.as_str(), who.team.as_str()), ("David", "Acme"));
        assert!(parse_auth_info(&serde_json::json!({ "ok": true })).is_err());
    }
}
