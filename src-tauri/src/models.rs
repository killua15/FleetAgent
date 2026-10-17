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
    /// Where a `custom` account points Claude Code. `None` for every other
    /// provider, whose endpoint is the CLI's own.
    pub base_url: Option<String>,
    /// The model a `custom` account asks its endpoint for, when set.
    pub default_model: Option<String>,
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
    /// A config dir that already exists on disk and is already logged in
    /// (e.g. `~/.claude-work`), adopted in place instead of running a fresh
    /// login. Takes precedence over `login_id`. Nothing is moved — the OAuth
    /// token in the keychain is keyed by this literal path.
    pub config_dir_path: Option<String>,
    pub monthly_cap: Option<f64>,
    /// Required for a `custom` account and ignored for any other provider.
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
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
/// Hangar's own recorded usage, not a live pull of the provider's real quota.
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

fn default_true() -> bool {
    true
}

/// App-wide preferences from the `app_settings` key/value table, collapsed
/// into one struct — the reviewer feature ("Review now") plus which events
/// fire a real macOS notification, all opt-in (default `false`) so enabling
/// the first one is the moment Hangar first asks the OS for permission,
/// rather than surprising the user at first launch.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub reviewer_enabled: bool,
    pub reviewer_account_id: Option<String>,
    pub notify_needs_approval: bool,
    pub notify_session_waiting: bool,
    pub notify_account_exhausted: bool,
    pub notify_spend_cap: bool,
    /// Covers a new PR being discovered, not just a new comment on one
    /// already tracked — one toggle, both are "a PR needs your attention".
    pub notify_pr_comments: bool,
    pub notify_update_available: bool,
    /// A task assigned to you appeared or changed status on a board — what the
    /// Tasks panel's background read finds. `default` so a frontend that
    /// predates it can still save settings.
    #[serde(default)]
    pub notify_task_changes: bool,
    /// Closing the window hides Hangar to the menu bar / tray instead of
    /// quitting, so the agents and the background polls keep running. On by
    /// default; Quit is in the tray menu and ⌘Q.
    #[serde(default = "default_true")]
    pub keep_in_menu_bar: bool,
    /// Opt-in gate for the tree-canvas feature (hierarchical agent teams) —
    /// off by default so the "Hierarchy trees" entry point in `AppTopBar`
    /// doesn't even show until a user deliberately turns it on.
    pub agent_trees_enabled: bool,
    /// Off by default: `check_pr_comments_for` normally drops any PR comment
    /// written by the logged-in `gh` user themselves, since the feature
    /// exists to surface *other* reviewers' feedback — but that makes it
    /// impossible to test solo without a second GitHub account. Turning
    /// this on keeps self-authored comments too.
    pub include_own_pr_comments: bool,
    /// How many open panels before the terminals view warns that they are
    /// getting hard to supervise at a glance.
    ///
    /// **`0` means never.** A magic value rather than an `Option` because this
    /// round-trips through `app_settings`, which is text key/value — and a
    /// threshold of zero has no other sensible reading, since a warning that
    /// fires at zero panels would fire before anything is open.
    ///
    /// Advisory only: nothing is blocked and no panel is folded automatically,
    /// which is what the banner's own wording already promises.
    pub panel_warn_threshold: i64,
}

/// One row of the Rotation settings screen's checklist — an account plus
/// whether the given project may use it, joined so the frontend doesn't
/// need a second round-trip to show name/provider/scope next to each box.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAccountRotation {
    pub account_id: String,
    pub account_name: String,
    pub provider: String,
    pub scope: String,
    pub allowed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGroup {
    pub id: String,
    pub name: String,
    pub created_at: String,
}

/// Memory group list-item shape — includes counts so the sidebar can show
/// "N projects · M memory files" without a separate query per group.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryGroupSummary {
    pub id: String,
    pub name: String,
    pub project_count: i64,
    pub file_count: i64,
}

/// One project's membership state for a given memory group.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMembership {
    pub project_id: String,
    pub project_name: String,
    pub member: bool,
}

/// A memory file's content — returned in full since the viewer shows/edits
/// the whole markdown body at once (small files, no need to paginate).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFile {
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub filename: String,
    pub content_md: String,
    pub updated_at: String,
}

/// Lighter shape for the file list in the sidebar (no content_md, which
/// could be large and isn't needed until a file is actually opened).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryFileSummary {
    pub id: String,
    pub project_id: String,
    pub project_name: String,
    pub filename: String,
    pub updated_at: String,
}

/// A real `.md` file already sitting on disk in a project's directory that
/// hasn't been imported into `memory_files` yet — surfaced so Shared Memory
/// can offer it as a one-click import instead of starting blank.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredMemoryFile {
    pub project_id: String,
    pub project_name: String,
    pub relative_path: String,
}

/// One node in a tree-canvas hierarchy (`agent_nodes` table) — a role/prompt/
/// model/account template, up to 3 levels deep (`depth`) via `parent_id`.
/// Running the tree creates a real `Session` per node (linked back via
/// `Session.node_id`); the node itself is just the reusable definition.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNode {
    pub id: String,
    pub project_id: String,
    pub parent_id: Option<String>,
    pub role: String,
    pub prompt: String,
    pub model: String,
    pub account_id: Option<String>,
    pub depth: i64,
    pub created_at: String,
}

/// An open PR on one of the projects' repos — one row per `pull_requests`.
/// Either one a session here pushed via `gh`, or one discovered on GitHub,
/// including PRs other people opened.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestInfo {
    pub id: String,
    /// The project whose repo this PR is on — the anchor, always present.
    pub project_id: String,
    /// The session that opened it, when Hangar did. `None` for a discovered
    /// PR, which is every PR that isn't yours and most that are.
    pub session_id: Option<String>,
    pub repo_owner: String,
    pub repo_name: String,
    pub number: i64,
    pub url: String,
    pub state: String,
    pub last_comment_seen_at: Option<String>,
    /// The error from the most recent failed check for this PR, if any — see
    /// `pull_requests.last_check_error` in schema.sql for why this exists.
    pub last_check_error: Option<String>,
    pub created_at: String,
    /// GitHub's `updatedAt` as of the last successful check — an internal
    /// freshness cache, not something the frontend needs.
    #[serde(skip)]
    pub last_seen_updated_at: Option<String>,
    /// The PR's actual head branch — `None` until either Hangar created the
    /// PR itself or `resolve_session_for_pr_branch` has backfilled it.
    pub head_branch: Option<String>,
    /// The branch the PR targets — what a review reads the diff against.
    pub base_branch: Option<String>,
    pub title: Option<String>,
    /// The PR author's GitHub login. `None` for rows that predate discovery
    /// recording it (all of which are the user's own).
    pub author_login: Option<String>,
    /// `mine`, `review` (requested from me or assigned to me) or `other`.
    /// Recomputed on every discovery pass — being asked to review a PR that
    /// was `other` yesterday is the normal course of a week.
    pub relation: String,
    /// `github` or `gitlab` — what the screens read to say "merge request"
    /// and `!42` instead of "pull request" and `#42`.
    pub forge: String,
    /// The instance this lives on: `github.com`, `gitlab.acme.com`. Sent to
    /// the frontend because "which GitLab" is part of identifying a row when a
    /// team has both a hosted and a self-hosted one.
    pub host: String,
}

/// A read-only preview of what `create_pull_request` would produce for a
/// session, computed without any of its mutations (no branch creation, no
/// commit, no push) — see `preview_pull_request` in `pull_requests.rs` for
/// how the diff is derived so this can't drift from what actually gets
/// created.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestPreview {
    pub suggested_title: String,
    pub body: String,
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub removed: Vec<String>,
    pub lines_added: i64,
    pub lines_removed: i64,
}

/// One comment fetched from a PR — either a general comment/review (from
/// `gh pr view --json comments,reviews`) or an inline diff comment tied to a
/// specific file/line (from `gh api .../pulls/{n}/comments`), collapsed into
/// one shape so the frontend/prompt-builder doesn't need to special-case
/// which kind it is; `path`/`line` are `None` for general comments. `id` is
/// the real `pr_review_comments` row once persisted — every comment that
/// reaches the frontend (banner, poller event, inbox screen) has one, so
/// "send"/"dismiss" always act on a real row instead of trusting
/// client-supplied text.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    pub path: Option<String>,
    pub line: Option<i64>,
}

/// One open PR (with its still-pending review comments) inside a project's
/// group in the PR review inbox screen.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrInboxEntry {
    pub pull_request: PullRequestInfo,
    /// The session behind this PR, when there is one. A PR opened by someone
    /// else has none — the card shows its author instead.
    pub session_id: Option<String>,
    pub session_name: Option<String>,
    pub comments: Vec<PrComment>,
}

/// When one forge instance was last polled for open PRs.
///
/// What lets the PR screen paint from the database and still say how old that
/// is — and, when `error` is set, say why it isn't newer.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrHostCheck {
    pub host: String,
    pub checked_at: String,
    pub error: Option<String>,
}

/// One project's group in the PR review inbox — every open PR that project
/// has, each with its pending (unresolved) comments.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectPrInbox {
    pub project_id: String,
    pub project_name: String,
    pub entries: Vec<PrInboxEntry>,
}

/// One worktree of a project shown in the git panel, with its live git status
/// — computed on demand, not cached. Usually a subsession's (a real `git
/// worktree`, or a copied folder with its own fresh git init), but also one git
/// knows about that no session owns: debris from a failed `worktree add`, or
/// one another tool made.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreeGitStatus {
    /// `None` for an orphan — a worktree git lists but no session owns.
    pub session_id: Option<String>,
    /// The session's name, or the directory's for an orphan.
    pub session_name: String,
    /// The session's account — what its terminal starts the CLI as. `None`
    /// for an orphan.
    pub account_id: Option<String>,
    pub branch: Option<String>,
    pub kind: String,
    pub path: String,
    pub ahead: i64,
    pub behind: i64,
    pub dirty: bool,
    /// The open PR tracked against this exact worktree's branch, if any —
    /// lets the git panel surface "Send to agent" review comments right
    /// next to the worktree they belong to.
    pub linked_pr: Option<PullRequestInfo>,
    /// The directory is gone: a session still points at it, or git still has
    /// it registered. No git status is computed for it; prune clears it.
    pub missing: bool,
    /// Outside `<project>/.hangar/worktrees`, so most likely another tool's.
    pub external: bool,
}

/// What `prune_worktrees` cleaned up, for the toast that reports it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorktreePruneResult {
    /// Registrations git dropped because their directory was gone.
    pub pruned_registrations: usize,
    /// Sessions whose worktree folder was gone, now pointed back at the project.
    pub cleared_sessions: usize,
}

/// One project's repo status in the git panel: its own root checkout plus
/// every active worktree branched off it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectGitOverview {
    pub project_id: String,
    pub project_name: String,
    pub project_path: String,
    /// `None` when the project's own directory isn't a git repo at all (a
    /// "copy"-only project) — the root card still shows, just without
    /// branch/ahead-behind info.
    pub repo_owner: Option<String>,
    pub repo_name: Option<String>,
    /// Which hosting product `origin` points at (`github`/`gitlab`) and which
    /// instance — the same pair `pull_requests.forge`/`.host` stores, so the
    /// git panel and the PR inbox can be filtered by one shared value. `None`
    /// when the project isn't a git repo, or when the host says nothing about
    /// itself (see `forges::Forge::for_host`).
    pub forge: Option<String>,
    pub host: Option<String>,
    pub default_branch: Option<String>,
    pub current_branch: Option<String>,
    pub ahead: i64,
    pub behind: i64,
    pub dirty: bool,
    pub worktrees: Vec<WorktreeGitStatus>,
}

/// One local or remote-tracking branch, for the git panel's "History" view —
/// `ahead`/`behind` come straight from `%(upstream:track)`, not a separate
/// `rev-list` call per branch, so listing every branch stays a single `git`
/// subprocess call even in a repo with many of them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    /// Short name without the `refs/heads/`/`refs/remotes/` prefix, e.g.
    /// `main` or `origin/feature-x`.
    pub name: String,
    pub is_remote: bool,
    pub is_current: bool,
    pub upstream: Option<String>,
    pub ahead: i64,
    pub behind: i64,
    pub last_commit_hash: String,
    pub last_commit_subject: String,
    pub last_commit_date: String,
}

/// One commit row for the git panel's commit graph — `parents` (full hashes)
/// is what a lane-layout algorithm needs to draw the DAG; `refs` are the
/// branch/tag names (if any) pointing directly at this commit, for the
/// "pill" labels shown inline (mirrors what VS Code's Git Graph shows).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitInfo {
    pub hash: String,
    pub short_hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub refs: Vec<String>,
}

/// One page of `list_commits` — `has_more` is derived from requesting one
/// extra row past the caller's limit, so the frontend knows whether to show
/// a "load more" control without a separate count query.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitLogPage {
    pub commits: Vec<CommitInfo>,
    pub has_more: bool,
}

/// One file changed by a commit (diffed against its first parent) — reuses
/// the same `FileStatus` enum/color convention the session file tree already
/// uses, so the frontend's existing `FILE_STATUS_COLOR` map applies as-is.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileChange {
    pub path: String,
    pub status: FileStatus,
}

/// A single file's before/after content for one commit — fed straight into
/// the frontend's existing pure `diffLines(oldText, newText)` differ, same
/// as a session's live file edits.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileDiff {
    pub old_text: String,
    pub new_text: String,
}
