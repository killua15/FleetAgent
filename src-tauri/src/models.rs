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
    pub parent_session_id: Option<String>,
    pub worktree_path: Option<String>,
    pub worktree_branch: Option<String>,
    pub worktree_kind: Option<String>,
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
    pub parent_session_id: Option<String>,
    pub worktree_branch: Option<String>,
    pub worktree_kind: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSubsession {
    pub parent_session_id: String,
    pub name: String,
    pub account_id: String,
    pub model: String,
    pub effort: String,
    pub mode: String,
    /// Shell command to run once inside the new worktree/copy (e.g. `npm
    /// install`) — optional, remembered on the project for next time.
    pub setup_command: Option<String>,
}

/// The real before/after content of one Edit/Write/MultiEdit tool call made
/// during a session, captured by `turn_runner` and used to render a diff.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEdit {
    pub id: String,
    pub session_id: String,
    pub block_id: Option<String>,
    pub file_path: String,
    pub tool_name: String,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
    pub created_at: String,
}

/// Token/cost accounting for one completed turn, captured from the `"result"`
/// message's `usage`/`total_cost_usd` fields and linked back to the `agent`
/// block it belongs to, for a per-turn token badge and session/context bars.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnUsage {
    pub id: String,
    pub session_id: String,
    pub block_id: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: Option<i64>,
    pub cache_creation_tokens: Option<i64>,
    pub cost_usd: Option<f64>,
    pub created_at: String,
}

/// Aggregate spend for one account over a trailing window (e.g. 7 days) —
/// Fleet's own recorded usage, not a live pull of the provider's real quota.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageSummary {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

/// One entry in a session's working-directory file tree, annotated with its
/// git (or best-effort touched-file) status so the UI can color it the way
/// VS Code does: green new/untracked, amber modified, red deleted.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTreeNode {
    /// Path relative to the session's working directory (worktree or project root).
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub status: Option<FileStatus>,
    pub children: Vec<FileTreeNode>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Untracked,
    /// No git repo to ask, but this path showed up in this session's own
    /// `file_edits` — the best signal available for a copy-kind subsession.
    Touched,
}
