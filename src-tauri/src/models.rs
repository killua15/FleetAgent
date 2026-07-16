use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider: String,
    pub billing: String,
    pub name: String,
    pub scope: String,
    pub plan_label: String,
    pub config_dir_path: String,
    pub credential_ref: String,
    pub monthly_cap: Option<f64>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub node_id: Option<String>,
    pub account_id: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub status: String,
    pub pid: Option<i64>,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub id: String,
    pub name: String,
    pub project_id: String,
    pub project_name: String,
    pub account_id: String,
    pub account_name: String,
    pub provider: String,
    pub scope: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    pub status: String,
    pub started_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBlock {
    pub id: String,
    pub session_id: String,
    pub kind: String,
    pub text: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    pub provider: String,
    pub billing: String,
    pub name: String,
    pub scope: String,
    pub plan_label: String,
    /// Raw secret (API key). Stored in the OS keychain, never persisted to
    /// SQLite — only its reference (the account id) is. `None` for
    /// subscription accounts, which authenticate via the isolated config dir.
    pub secret: Option<String>,
    /// For subscription accounts: the completed `begin_login` id whose
    /// pending config dir (already holding valid credentials) should be
    /// adopted as this account's permanent config dir.
    pub login_id: Option<String>,
    pub monthly_cap: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    /// The literal value to pass to the CLI's `--model` flag (an alias like
    /// "sonnet" or a full model id like "claude-opus-4-8").
    pub id: String,
    pub label: String,
}

/// A past Claude Code conversation found on disk for a session's project,
/// available to resume into instead of the session's own conversation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResumableConversation {
    pub provider_session_id: String,
    pub preview: String,
    pub last_active_at: String,
}
