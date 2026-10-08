//! Account rotation: keeping a session working when the account behind it
//! runs out of quota.
//!
//! The schema has modelled this since the beginning (`rotation_policies`,
//! `rotation_events`, `project_allowed_accounts`) and nothing ever read those
//! tables — an account that hit a rate limit got flagged `exhausted`, fired a
//! notification, and then sat there while every subsequent turn kept aiming
//! at the same dead account. This module is the missing half.
//!
//! **What rotation does NOT do**: it never overrides an explicit, working
//! choice. The account picked in the UI when a session starts is used as long
//! as it can actually run. Rotation only engages when the current account
//! *can't* — which makes it a failover mechanism, not a scheduler. (An
//! earlier reading of the UI copy — "picks the least-loaded allowed account
//! when a session starts" — would have silently reassigned every new session
//! away from the account the user just chose.)
//!
//! **Opt-in, per project**: candidates come only from
//! `project_allowed_accounts` (checked in Settings → Rotation) and only from
//! the same provider, and the default policy is `manual`, which never
//! rotates. Nothing is shared across projects — or across the work/personal
//! boundary — unless the user checked the box.

use crate::db::DbConnection;
use chrono::{Duration, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

/// How long an `exhausted` account is left alone before rotation will
/// consider it again. Claude's headline subscription limit is a rolling
/// 5-hour window, so an account that tripped it is plausibly usable again
/// after one. This is deliberately a *recovery* heuristic, not a claim to
/// know the real reset time — Hangar has no API for that (see
/// `usage_scraper`), and the cost of guessing early is one failed turn that
/// re-flags the account, not data loss.
const EXHAUSTION_COOLDOWN_HOURS: i64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Policy {
    /// Never rotate. The default, and what every project has until someone
    /// changes it in Settings → Rotation.
    Manual,
    /// Least-loaded: the allowed account with the least recorded usage over
    /// the trailing 7 days.
    Auto,
    /// Least-recently-used: the allowed account that went longest without
    /// being rotated to, so a pool gets cycled evenly rather than always
    /// falling to whichever account happens to be cheapest.
    RoundRobin,
}

impl Policy {
    pub fn from_str(s: &str) -> Self {
        match s {
            "auto" => Policy::Auto,
            "round_robin" => Policy::RoundRobin,
            _ => Policy::Manual,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Policy::Auto => "auto",
            Policy::RoundRobin => "round_robin",
            Policy::Manual => "manual",
        }
    }
}

/// One account rotation could switch to, with the two signals the policies
/// rank by already resolved.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub account_id: String,
    pub account_name: String,
    /// Trailing-7-day recorded spend (Hangar's own `turn_usage` numbers, not a
    /// live provider figure) — the "load" in least-loaded.
    pub recent_cost_usd: f64,
    /// Recorded tokens over the same window. Subscription accounts never
    /// report a dollar cost, so for a pool of those `recent_cost_usd` is 0.0
    /// across the board and this is what actually discriminates.
    pub recent_tokens: i64,
    /// When this account was last rotated *to* on this project, from
    /// `rotation_events`. `None` means never, which sorts first — an unused
    /// account is the most "least-recently-used" one there is.
    pub last_used_at: Option<String>,
}

/// Picks the account to switch to, or `None` to stay put. Pure and
/// side-effect free so the ranking rules are testable without a database.
///
/// Ties break on `account_id` in every policy, purely so the choice is
/// deterministic: without it, two equally-idle accounts would be picked in
/// whatever order SQLite happened to return them, and the same failure would
/// rotate somewhere different each time.
pub fn pick(policy: Policy, candidates: &[Candidate]) -> Option<&Candidate> {
    match policy {
        Policy::Manual => None,
        Policy::Auto => candidates.iter().min_by(|a, b| {
            a.recent_cost_usd
                .total_cmp(&b.recent_cost_usd)
                .then(a.recent_tokens.cmp(&b.recent_tokens))
                .then(a.account_id.cmp(&b.account_id))
        }),
        Policy::RoundRobin => candidates.iter().min_by(|a, b| {
            // `None` (never used) must sort before any timestamp; `Option`'s
            // own Ord already does exactly that.
            a.last_used_at.cmp(&b.last_used_at).then(a.account_id.cmp(&b.account_id))
        }),
    }
}

/// Reads the policy for a (project, provider) pair. Absent row = `manual`,
/// matching the column default and the "nothing rotates unless you asked"
/// rule.
pub fn read_policy(conn: &Connection, project_id: &str, provider: &str) -> Policy {
    conn.query_row(
        "SELECT policy FROM rotation_policies WHERE project_id = ?1 AND provider = ?2",
        params![project_id, provider],
        |row| row.get::<_, String>(0),
    )
    .map(|s| Policy::from_str(&s))
    .unwrap_or(Policy::Manual)
}

/// Clears the `exhausted` flag from any account whose cooldown has elapsed.
/// Without this, rotation is a one-way ratchet: every account that ever hit a
/// rate limit stays permanently ineligible, and a pool of N accounts degrades
/// to zero over a few days with no way back short of editing the database.
///
/// Reads `exhausted_at`, not `updated_at`. It used to read `updated_at`, which
/// made recovery hostage to unrelated writes: repointing an account's config
/// dir refreshed that column and pushed the cooldown out by however long the
/// account had already been benched. A NULL `exhausted_at` on a benched account
/// means its age is unknown — a flag inherited from before the column existed —
/// and counts as expired, so it clears rather than sticking forever.
///
/// Called from the terminal usage poller as well as `rotate_session_if_needed`.
/// Only the latter used to call it, and with the pivot to terminals nothing
/// called it at all — which is how an account benched by a headless turn stayed
/// benched across restarts, showing an out-of-quota banner on a terminal whose
/// account was fine.
/// Returns how many accounts it lifted, so a caller that shows account state in
/// the UI knows whether it has to refresh.
pub(crate) fn recover_cooled_down_accounts(conn: &Connection) -> Result<usize, String> {
    let cutoff = (Utc::now() - Duration::hours(EXHAUSTION_COOLDOWN_HOURS)).to_rfc3339();
    let recovered = conn
        .execute(
            "UPDATE accounts SET status = 'healthy', exhausted_at = NULL, updated_at = ?2
             WHERE status = 'exhausted' AND (exhausted_at IS NULL OR exhausted_at <= ?1)",
            params![cutoff, Utc::now().to_rfc3339()],
        )
        .map_err(|e| e.to_string())?;
    if recovered > 0 {
        log::info!("rotation: {recovered} account(s) came off cooldown and are eligible again");
    }
    Ok(recovered)
}

/// Every account this project is allowed to rotate to for `provider`, minus
/// the one it's already on, ranked-ready.
///
/// The `provider` filter is not optional: an account's credentials, config
/// dir, and model names are all provider-specific, so "rotate to the
/// least-loaded account" only ever means "…among accounts speaking the same
/// CLI".
fn load_candidates(conn: &Connection, project_id: &str, provider: &str, exclude_account_id: &str) -> Result<Vec<Candidate>, String> {
    let since = (Utc::now() - Duration::days(7)).to_rfc3339();
    let mut stmt = conn
        .prepare(
            "SELECT a.id,
                    a.name,
                    COALESCE((SELECT SUM(tu.cost_usd) FROM turn_usage tu
                                JOIN sessions s ON s.id = tu.session_id
                               WHERE s.account_id = a.id AND tu.created_at >= ?3), 0.0),
                    COALESCE((SELECT SUM(tu.input_tokens + tu.output_tokens) FROM turn_usage tu
                                JOIN sessions s ON s.id = tu.session_id
                               WHERE s.account_id = a.id AND tu.created_at >= ?3), 0),
                    (SELECT MAX(re.created_at) FROM rotation_events re
                      WHERE re.to_account_id = a.id AND re.project_id = ?1)
             FROM accounts a
             JOIN project_allowed_accounts paa ON paa.account_id = a.id AND paa.project_id = ?1
             WHERE paa.allowed = 1
               AND a.provider = ?2
               AND a.status = 'healthy'
               AND a.id <> ?4",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id, provider, since, exclude_account_id], |row| {
            Ok(Candidate {
                account_id: row.get(0)?,
                account_name: row.get(1)?,
                recent_cost_usd: row.get(2)?,
                recent_tokens: row.get(3)?,
                last_used_at: row.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// What `rotate_session_if_needed` did, for the caller to report in the
/// session transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Switch {
    pub to_account_id: String,
    pub to_account_name: String,
    pub from_account_name: String,
    /// True when the switch also dropped the session's `--resume` id — see
    /// `rotate_session_if_needed` for why that's unavoidable.
    pub lost_conversation: bool,
}

/// Moves `session_id` onto a healthy allowed account if the one it's on can't
/// run, and records the switch in `rotation_events`. Returns `None` when
/// nothing needed to change — which is the overwhelmingly common case, so
/// this is cheap to call before every turn.
///
/// **Conversation continuity is dropped on a switch.** Claude Code's
/// `--resume <id>` reads a transcript stored under the account's own
/// `CLAUDE_CONFIG_DIR`; the new account has a different config dir and has
/// never seen that id, so passing it would fail outright. `provider_session_id`
/// is therefore cleared and the caller tells the user, rather than silently
/// starting a fresh conversation that looks like amnesia.
pub fn rotate_session_if_needed(conn: &Connection, session_id: &str) -> Result<Option<Switch>, String> {
    recover_cooled_down_accounts(conn)?;

    let (project_id, account_id, account_name, account_status, provider, provider_session_id): (String, String, String, String, String, Option<String>) = conn
        .query_row(
            "SELECT s.project_id, a.id, a.name, a.status, a.provider, s.provider_session_id
             FROM sessions s JOIN accounts a ON a.id = s.account_id
             WHERE s.id = ?1",
            params![session_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .map_err(|e| format!("session lookup failed: {e}"))?;

    // The account can still run — rotation stays out of the way. This is the
    // "never override a working explicit choice" rule.
    if account_status == "healthy" {
        return Ok(None);
    }

    let policy = read_policy(conn, &project_id, &provider);
    if policy == Policy::Manual {
        log::info!("rotation: session {session_id} is on an {account_status} account but its project policy is manual — staying put");
        return Ok(None);
    }

    let candidates = load_candidates(conn, &project_id, &provider, &account_id)?;
    let Some(chosen) = pick(policy, &candidates) else {
        log::warn!("rotation: session {session_id} needs a new account but no allowed healthy '{provider}' account is available");
        return Ok(None);
    };

    conn.execute(
        "UPDATE sessions SET account_id = ?2, provider_session_id = NULL WHERE id = ?1",
        params![session_id, chosen.account_id],
    )
    .map_err(|e| e.to_string())?;

    conn.execute(
        "INSERT INTO rotation_events (id, project_id, from_account_id, to_account_id, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            Uuid::new_v4().to_string(),
            project_id,
            account_id,
            chosen.account_id,
            format!("{account_status} ({} policy)", policy.as_str()),
            Utc::now().to_rfc3339(),
        ],
    )
    .map_err(|e| e.to_string())?;

    log::info!(
        "rotation: session {session_id} switched from '{account_name}' ({account_status}) to '{}' via {} policy",
        chosen.account_name,
        policy.as_str()
    );

    Ok(Some(Switch {
        to_account_id: chosen.account_id.clone(),
        to_account_name: chosen.account_name.clone(),
        from_account_name: account_name,
        lost_conversation: provider_session_id.is_some(),
    }))
}

/// The human-readable note dropped into the session transcript when a switch
/// happens, so the account change is never silent — it changes which quota is
/// being spent and, when a conversation is dropped, what the agent remembers.
pub fn switch_notice(switch: &Switch) -> String {
    let mut text = format!(
        "↻ switched from account `{}` (rate-limited) to `{}` — this project allows rotating between them.",
        switch.from_account_name, switch.to_account_name
    );
    if switch.lost_conversation {
        text.push_str(
            "\n\nThe previous conversation could not be carried over: Claude Code stores it under the old account's own config directory, \
             so this turn starts a fresh conversation. Re-state any context the agent still needs.",
        );
    }
    text
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// Reads the rotation policy for a project/provider pair, for the selector in
/// Settings → Rotation.
#[tauri::command]
pub fn get_rotation_policy(db: tauri::State<DbConnection>, project_id: String, provider: String) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(read_policy(&conn, &project_id, &provider).as_str().to_string())
}

/// The account the out-of-quota banner offers to reopen a terminal under.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestedAccount {
    pub account_id: String,
    pub account_name: String,
    /// Set when this is the project's fallback rather than another account of
    /// the same provider: the host the project's code will be sent to, so the
    /// banner can say so before the click instead of after.
    pub fallback_host: Option<String>,
}

/// The provider a project's fallback stands in for. Only Claude: a fallback is
/// Claude Code pointed at another endpoint, so it can take over a Claude
/// terminal — its worktree, its flags, its transcript — and nothing else's.
const FALLBACK_HOME_PROVIDER: &str = "claude";

/// The host part of an endpoint address, for showing where code will go.
/// The whole address when it has no recognisable host, so nothing is hidden.
pub(crate) fn endpoint_host(base_url: &str) -> String {
    let rest = base_url.split_once("://").map_or(base_url, |(_, rest)| rest);
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    if host.is_empty() { base_url.to_string() } else { host.to_string() }
}

/// A project's fallback account, resolved.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Fallback {
    pub account_id: String,
    pub account_name: String,
    pub host: String,
    pub status: String,
}

/// The account `project_id` falls back to, whatever state it is in — `None`
/// when the project never picked one, or picked one that has since lost its
/// endpoint.
pub(crate) fn project_fallback(conn: &Connection, project_id: &str) -> Result<Option<Fallback>, String> {
    let row = conn.query_row(
        "SELECT a.id, a.name, a.base_url, a.status
         FROM projects p JOIN accounts a ON a.id = p.fallback_account_id
         WHERE p.id = ?1 AND a.provider = 'custom' AND a.base_url IS NOT NULL",
        params![project_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
    );
    match row {
        Ok((account_id, account_name, base_url, status)) => Ok(Some(Fallback { account_id, account_name, host: endpoint_host(&base_url), status })),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("fallback account lookup failed: {e}")),
    }
}

/// What a terminal on `exhausted_account_id` should be offered: another allowed
/// account of the same provider when there is one, and the project's fallback
/// only when there is not.
///
/// That order is the whole contract. A fallback sends the project's code to a
/// different company and usually a weaker model, so it is never preferred over
/// an account that would have kept everything as it was.
fn suggest_for_terminal(conn: &Connection, project_id: &str, exhausted_account_id: &str) -> Result<Option<SuggestedAccount>, String> {
    let candidates = load_candidates(conn, project_id, FALLBACK_HOME_PROVIDER, exhausted_account_id)?;
    let policy = match read_policy(conn, project_id, FALLBACK_HOME_PROVIDER) {
        Policy::Manual => Policy::Auto,
        other => other,
    };
    if let Some(c) = pick(policy, &candidates) {
        return Ok(Some(SuggestedAccount { account_id: c.account_id.clone(), account_name: c.account_name.clone(), fallback_host: None }));
    }
    Ok(project_fallback(conn, project_id)?
        .filter(|f| f.status == "healthy" && f.account_id != exhausted_account_id)
        .map(|f| SuggestedAccount { account_id: f.account_id, account_name: f.account_name, fallback_host: Some(f.host) }))
}

/// The project a terminal target belongs to: a project target is one directly;
/// a session target (e.g. a PR worktree) resolves through its session row.
pub(crate) fn project_of_terminal(conn: &Connection, kind: &str, source_id: &str) -> Result<String, String> {
    match kind {
        "project" => Ok(source_id.to_string()),
        "session" => conn
            .query_row("SELECT project_id FROM sessions WHERE id = ?1", params![source_id], |r| r.get(0))
            .map_err(|e| format!("terminal's session lookup failed: {e}")),
        other => Err(format!("unknown terminal target kind '{other}'")),
    }
}

/// Which allowed, healthy account a terminal should reopen under once its own
/// account is out of quota — the banner's "reopen in <Y>" suggestion.
///
/// Uses the project's rotation policy to rank, except that `Manual` falls back
/// to least-loaded here rather than declining: in the terminal model the user
/// already makes the call by clicking the button, so "manual" only means "no
/// *automatic* switch", not "never suggest one". `None` when the project has
/// no other allowed healthy Claude account and no fallback — the banner then
/// warns without a button.
#[tauri::command]
pub fn suggest_terminal_account(
    db: tauri::State<DbConnection>,
    kind: String,
    source_id: String,
    exhausted_account_id: String,
) -> Result<Option<SuggestedAccount>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let project_id = project_of_terminal(&conn, &kind, &source_id)?;
    suggest_for_terminal(&conn, &project_id, &exhausted_account_id)
}

/// One row of the account switcher on a terminal panel's header chip.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotatableAccount {
    pub account_id: String,
    pub account_name: String,
    pub scope: String,
    /// Straight from the accounts row (`healthy`, `exhausted`, …). A benched
    /// account is listed and disabled rather than hidden: the pool the user
    /// checked in Settings → Rotation should be the pool they see, with the
    /// reason one of them can't be picked right now.
    pub status: String,
    pub current: bool,
    /// The one this project's rotation policy would pick. Shows the policy
    /// where the decision actually gets made, not only in Settings.
    pub suggested: bool,
    /// For the project's fallback account: the host its endpoint lives on.
    /// `None` on every ordinary row.
    pub fallback_host: Option<String>,
}

/// Every account the switcher offers for a terminal currently on
/// `current_account_id`: the project's allowed accounts of the same provider,
/// plus the current one whether or not it's checked.
///
/// Deliberately wider than `load_candidates` in two ways — it keeps benched
/// accounts (the UI disables them) and it keeps the account you're on (so the
/// list can say where you are). Narrower in none: the provider filter is as
/// non-negotiable here as it is there, because a terminal can only reopen under
/// a CLI Hangar can run it as.
///
/// Split from the command so the ordering and membership rules are testable
/// without a mock app.
fn rotatable_accounts(conn: &Connection, project_id: &str, provider: &str, current_account_id: &str) -> Result<Vec<RotatableAccount>, String> {
    // Same fallback as `suggest_terminal_account`: clicking a row *is* the
    // manual decision, so `manual` still means "no automatic switch", not "no
    // recommendation".
    let policy = match read_policy(conn, project_id, provider) {
        Policy::Manual => Policy::Auto,
        other => other,
    };
    let candidates = load_candidates(conn, project_id, provider, current_account_id)?;
    let suggested = pick(policy, &candidates).map(|c| c.account_id.clone());

    // The fallback rides along as the last row, and only for the provider it
    // can stand in for. It is never `suggested` from here: that mark means "the
    // policy would pick this", and the policy only ranks same-provider accounts.
    let fallback = if provider == FALLBACK_HOME_PROVIDER { project_fallback(conn, project_id)? } else { None };
    let fallback_id = fallback.as_ref().map(|f| f.account_id.clone());

    let mut stmt = conn
        .prepare(
            "SELECT a.id, a.name, a.scope, a.status
             FROM accounts a
             LEFT JOIN project_allowed_accounts paa ON paa.account_id = a.id AND paa.project_id = ?1
             WHERE (a.provider = ?2 AND COALESCE(paa.allowed, 0) = 1) OR a.id = ?3 OR a.id = ?4
             ORDER BY (a.id = ?3) DESC, (a.id IS ?4) ASC, a.scope, a.name",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id, provider, current_account_id, fallback_id], |row| {
            let account_id: String = row.get(0)?;
            Ok(RotatableAccount {
                current: account_id == current_account_id,
                suggested: suggested.as_deref() == Some(account_id.as_str()),
                fallback_host: fallback.as_ref().filter(|f| f.account_id == account_id).map(|f| f.host.clone()),
                account_id,
                account_name: row.get(1)?,
                scope: row.get(2)?,
                status: row.get(3)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// The accounts a terminal's header chip offers to switch to. Unlike the
/// out-of-quota banner, this is reachable while everything is fine — switching
/// account is a choice, not only a recovery.
#[tauri::command]
pub fn list_terminal_rotation_accounts(
    db: tauri::State<DbConnection>,
    kind: String,
    source_id: String,
    current_account_id: String,
) -> Result<Vec<RotatableAccount>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    // Benched accounts are re-checked lazily elsewhere (the usage poller); the
    // cheap part — lifting anyone whose cooldown simply elapsed — is worth doing
    // here so the list doesn't grey out an account that recovered hours ago.
    recover_cooled_down_accounts(&conn)?;

    let project_id = project_of_terminal(&conn, &kind, &source_id)?;
    let provider = switcher_provider(&conn, &project_id, &current_account_id)?;
    rotatable_accounts(&conn, &project_id, &provider, &current_account_id)
}

/// Which provider's pool the switcher shows for a terminal on this account.
///
/// Its own — except on the project's fallback, where the pool that matters is
/// the one it is standing in for. Listing only other custom accounts there
/// would leave a terminal on its fallback with no row to go back by.
fn switcher_provider(conn: &Connection, project_id: &str, current_account_id: &str) -> Result<String, String> {
    if project_fallback(conn, project_id)?.is_some_and(|f| f.account_id == current_account_id) {
        return Ok(FALLBACK_HOME_PROVIDER.to_string());
    }
    conn.query_row("SELECT provider FROM accounts WHERE id = ?1", params![current_account_id], |r| r.get(0))
        .map_err(|e| format!("terminal's account lookup failed: {e}"))
}

/// The accounts a project is allowed to rotate to that are *not* currently
/// usable — the mirror image of `load_candidates`, which only ever returns
/// healthy ones.
///
/// Exists so the usage poller can re-check them. A benched account with no
/// terminal open is never polled (that's deliberate — each check costs a hidden
/// `claude` in a PTY), so once flagged it stays flagged, and the out-of-quota
/// banner ends up saying "no other allowed account to switch to" about a pool
/// that recovered hours ago. Re-checking them only when an in-use account is
/// actually benched keeps that cost off the common path.
pub(crate) fn allowed_benched_accounts(conn: &Connection, project_id: &str, provider: &str, exclude_account_id: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT a.id FROM accounts a
             JOIN project_allowed_accounts paa ON paa.account_id = a.id AND paa.project_id = ?1
             WHERE paa.allowed = 1 AND a.provider = ?2 AND a.status != 'healthy' AND a.id != ?3",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id, provider, exclude_account_id], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

/// Why a terminal changed account, as recorded in `rotation_events`. Derived
/// from the account it left rather than from the button that was pressed: both
/// the header switcher and the out-of-quota banner call the same reopen, and
/// what makes the two cases different is whether the old account could still
/// run, not which control started it.
pub(crate) fn terminal_rotation_reason(conn: &Connection, from_account_id: &str) -> String {
    let status: String = conn
        .query_row("SELECT status FROM accounts WHERE id = ?1", params![from_account_id], |r| r.get(0))
        .unwrap_or_else(|_| "unknown".to_string());
    if status == "healthy" {
        "manual (switched from the panel)".to_string()
    } else {
        format!("{status} (terminal reopened by hand)")
    }
}

/// Records a terminal reopened under another account in `rotation_events`, the
/// same ledger `rotate_session_if_needed` writes to.
///
/// Not just bookkeeping: `load_candidates` reads this table as each account's
/// `last_used_at`, so without a row here a round-robin project would keep
/// suggesting the same account every time a terminal rotates, having no record
/// that it was already handed one.
pub(crate) fn record_terminal_rotation(conn: &Connection, kind: &str, source_id: &str, from_account_id: &str, to_account_id: &str) -> Result<(), String> {
    let project_id = project_of_terminal(conn, kind, source_id)?;
    let mut reason = terminal_rotation_reason(conn, from_account_id);
    // Said in the ledger because it is the one switch that changes who sees
    // the code, not just which quota is spent.
    if project_fallback(conn, &project_id)?.is_some_and(|f| f.account_id == to_account_id) {
        reason.push_str(" → fallback endpoint");
    }
    conn.execute(
        "INSERT INTO rotation_events (id, project_id, from_account_id, to_account_id, reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![Uuid::new_v4().to_string(), project_id, from_account_id, to_account_id, reason, Utc::now().to_rfc3339()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn set_rotation_policy(db: tauri::State<DbConnection>, project_id: String, provider: String, policy: String) -> Result<(), String> {
    // Round-trip through `Policy` rather than trusting the string: the column
    // has a CHECK constraint, and a typo from the frontend should land as a
    // sane default instead of a raw SQLite constraint error.
    let policy = Policy::from_str(&policy);
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO rotation_policies (project_id, provider, policy) VALUES (?1, ?2, ?3)
         ON CONFLICT(project_id, provider) DO UPDATE SET policy = excluded.policy",
        params![project_id, provider, policy.as_str()],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// The project's fallback account id, for Settings → Rotation.
#[tauri::command]
pub fn get_project_fallback_account(db: tauri::State<DbConnection>, project_id: String) -> Result<Option<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    Ok(project_fallback(&conn, &project_id)?.map(|f| f.account_id))
}

/// Sets or clears (`None`) the project's fallback account.
#[tauri::command]
pub fn set_project_fallback_account(db: tauri::State<DbConnection>, project_id: String, account_id: Option<String>) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    store_project_fallback(&conn, &project_id, account_id.as_deref())
}

/// Only a custom-endpoint account can be a fallback. Another Claude account
/// already has a place — the rotation pool — and an account driven through a
/// different CLI can't take over a Claude terminal at all.
fn store_project_fallback(conn: &Connection, project_id: &str, account_id: Option<&str>) -> Result<(), String> {
    if let Some(account_id) = account_id {
        let usable: bool = conn
            .query_row("SELECT provider = 'custom' AND base_url IS NOT NULL FROM accounts WHERE id = ?1", params![account_id], |r| r.get(0))
            .map_err(|e| format!("fallback account lookup failed: {e}"))?;
        if !usable {
            return Err("only a custom endpoint account can be a project's fallback".to_string());
        }
    }
    let changed = conn
        .execute("UPDATE projects SET fallback_account_id = ?2 WHERE id = ?1", params![project_id, account_id])
        .map_err(|e| e.to_string())?;
    if changed == 0 {
        return Err(format!("no project with id {project_id}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, cost: f64, tokens: i64, last_used: Option<&str>) -> Candidate {
        Candidate {
            account_id: id.to_string(),
            account_name: format!("Account {id}"),
            recent_cost_usd: cost,
            recent_tokens: tokens,
            last_used_at: last_used.map(str::to_string),
        }
    }

    #[test]
    fn manual_policy_never_rotates() {
        let candidates = vec![candidate("a", 0.0, 0, None)];
        assert_eq!(pick(Policy::Manual, &candidates), None);
    }

    #[test]
    fn no_candidates_means_no_switch_under_any_policy() {
        assert_eq!(pick(Policy::Auto, &[]), None);
        assert_eq!(pick(Policy::RoundRobin, &[]), None);
    }

    #[test]
    fn auto_policy_picks_the_cheapest_account() {
        let candidates = vec![candidate("a", 12.0, 500, None), candidate("b", 3.0, 900, None), candidate("c", 7.5, 100, None)];
        assert_eq!(pick(Policy::Auto, &candidates).unwrap().account_id, "b");
    }

    /// Subscription accounts never report a dollar cost, so a whole pool of
    /// them is 0.0 across the board — tokens are what actually rank them.
    #[test]
    fn auto_policy_falls_back_to_tokens_when_no_cost_is_recorded() {
        let candidates = vec![candidate("a", 0.0, 9_000, None), candidate("b", 0.0, 1_200, None)];
        assert_eq!(pick(Policy::Auto, &candidates).unwrap().account_id, "b");
    }

    #[test]
    fn round_robin_prefers_an_account_that_has_never_been_used() {
        let candidates = vec![candidate("a", 0.0, 0, Some("2026-01-01T00:00:00Z")), candidate("b", 99.0, 99_999, None)];
        assert_eq!(pick(Policy::RoundRobin, &candidates).unwrap().account_id, "b", "never-used must outrank any timestamp, regardless of load");
    }

    #[test]
    fn round_robin_picks_the_least_recently_used_account() {
        let candidates = vec![
            candidate("a", 0.0, 0, Some("2026-07-01T00:00:00Z")),
            candidate("b", 0.0, 0, Some("2026-05-01T00:00:00Z")),
            candidate("c", 0.0, 0, Some("2026-06-01T00:00:00Z")),
        ];
        assert_eq!(pick(Policy::RoundRobin, &candidates).unwrap().account_id, "b");
    }

    /// Two identical candidates must always resolve the same way — otherwise
    /// the same failure rotates somewhere different on every retry, which
    /// makes the rotation_events history impossible to reason about.
    #[test]
    fn ties_break_deterministically_on_account_id() {
        let candidates = vec![candidate("z", 1.0, 10, None), candidate("a", 1.0, 10, None)];
        assert_eq!(pick(Policy::Auto, &candidates).unwrap().account_id, "a");
        assert_eq!(pick(Policy::RoundRobin, &candidates).unwrap().account_id, "a");
    }

    #[test]
    fn policy_strings_round_trip_and_unknown_values_fall_back_to_manual() {
        for policy in [Policy::Manual, Policy::Auto, Policy::RoundRobin] {
            assert_eq!(Policy::from_str(policy.as_str()), policy);
        }
        assert_eq!(Policy::from_str("nonsense"), Policy::Manual);
        assert_eq!(Policy::from_str(""), Policy::Manual);
    }

    // -- DB-backed tests -----------------------------------------------------

    fn seed(policy: Option<&str>, allowed: &[&str], exhausted_at: Option<String>) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Test','/tmp','now')", []).unwrap();

        // acc1 is the exhausted one the session starts on; acc2/acc3 are
        // healthy alternatives; other1 is a different provider entirely.
        // Goes into `exhausted_at`, which is what recovery reads. It used to be
        // written to `updated_at`, back when recovery read that instead — the
        // overloading this column exists to undo.
        let benched_at = exhausted_at.unwrap_or_else(|| Utc::now().to_rfc3339());
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at, exhausted_at)
             VALUES ('acc1','claude','sub','Primary','work','','','acc1','exhausted','now','now',?1)",
            params![benched_at],
        )
        .unwrap();
        for id in ["acc2", "acc3"] {
            conn.execute(
                "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
                 VALUES (?1,'claude','sub',?1,'work','','',?1,'healthy','now','now')",
                params![id],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('other1','codex','sub','Codex','work','','','other1','healthy','now','now')",
            [],
        )
        .unwrap();

        for id in allowed {
            conn.execute(
                "INSERT INTO project_allowed_accounts (project_id, account_id, allowed) VALUES ('proj1', ?1, 1)",
                params![id],
            )
            .unwrap();
        }
        if let Some(policy) = policy {
            conn.execute(
                "INSERT INTO rotation_policies (project_id, provider, policy) VALUES ('proj1','claude',?1)",
                params![policy],
            )
            .unwrap();
        }
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, provider_session_id, started_at)
             VALUES ('sess1','s','proj1','acc1','opus','high','ask_edits','waiting','convo-abc','now')",
            [],
        )
        .unwrap();
        conn
    }

    fn session_account(conn: &Connection) -> String {
        conn.query_row("SELECT account_id FROM sessions WHERE id = 'sess1'", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn rotates_an_exhausted_session_onto_an_allowed_account_and_records_the_event() {
        let conn = seed(Some("auto"), &["acc2", "acc3"], None);

        let switch = rotate_session_if_needed(&conn, "sess1").unwrap().expect("an exhausted account with allowed alternatives must rotate");
        assert_eq!(switch.from_account_name, "Primary");
        assert_ne!(switch.to_account_id, "acc1");
        assert_eq!(session_account(&conn), switch.to_account_id, "the session row must actually be repointed");

        let (from, to): (String, String) = conn
            .query_row("SELECT from_account_id, to_account_id FROM rotation_events WHERE project_id = 'proj1'", [], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("the switch must be recorded in rotation_events");
        assert_eq!(from, "acc1");
        assert_eq!(to, switch.to_account_id);
    }

    /// The `--resume` id belongs to the old account's config dir and is
    /// meaningless to the new one, so it must be cleared — and the user must
    /// be told, which is what `lost_conversation` drives.
    #[test]
    fn rotating_clears_the_resume_id_and_flags_the_lost_conversation() {
        let conn = seed(Some("auto"), &["acc2"], None);

        let switch = rotate_session_if_needed(&conn, "sess1").unwrap().unwrap();
        assert!(switch.lost_conversation);
        assert!(switch_notice(&switch).contains("fresh conversation"));

        let resume: Option<String> = conn.query_row("SELECT provider_session_id FROM sessions WHERE id = 'sess1'", [], |r| r.get(0)).unwrap();
        assert_eq!(resume, None, "a stale --resume id would make the next turn fail outright");
    }

    #[test]
    fn a_healthy_account_is_never_rotated_away_from() {
        let conn = seed(Some("auto"), &["acc2", "acc3"], None);
        conn.execute("UPDATE accounts SET status = 'healthy' WHERE id = 'acc1'", []).unwrap();

        assert_eq!(rotate_session_if_needed(&conn, "sess1").unwrap(), None);
        assert_eq!(session_account(&conn), "acc1", "an explicit, working choice must be left alone");
    }

    #[test]
    fn manual_policy_leaves_an_exhausted_session_where_it_is() {
        let conn = seed(Some("manual"), &["acc2", "acc3"], None);
        assert_eq!(rotate_session_if_needed(&conn, "sess1").unwrap(), None);
        assert_eq!(session_account(&conn), "acc1");
    }

    /// The default with no `rotation_policies` row must be `manual` — this is
    /// the "nothing rotates unless you asked for it" guarantee.
    #[test]
    fn absent_policy_row_defaults_to_manual_and_does_not_rotate() {
        let conn = seed(None, &["acc2", "acc3"], None);
        assert_eq!(read_policy(&conn, "proj1", "claude"), Policy::Manual);
        assert_eq!(rotate_session_if_needed(&conn, "sess1").unwrap(), None);
    }

    /// Checking no boxes in Settings → Rotation means exactly that, even with
    /// a non-manual policy set and healthy accounts sitting right there.
    #[test]
    fn accounts_not_allowed_for_the_project_are_never_candidates() {
        let conn = seed(Some("auto"), &[], None);
        assert_eq!(rotate_session_if_needed(&conn, "sess1").unwrap(), None);
        assert_eq!(session_account(&conn), "acc1");
    }

    /// An account from another provider has different credentials, a
    /// different config dir, and different model names — it is never a valid
    /// substitute, even if the user allowed it for the project.
    #[test]
    fn a_different_providers_account_is_never_a_candidate() {
        let conn = seed(Some("auto"), &["other1"], None);
        assert_eq!(rotate_session_if_needed(&conn, "sess1").unwrap(), None);
        assert_eq!(session_account(&conn), "acc1");
    }

    /// Rotation must not be a one-way ratchet: once the cooldown passes, an
    /// exhausted account becomes usable again on its own.
    #[test]
    fn an_account_past_its_cooldown_recovers_and_needs_no_rotation() {
        let long_ago = (Utc::now() - Duration::hours(EXHAUSTION_COOLDOWN_HOURS + 1)).to_rfc3339();
        let conn = seed(Some("auto"), &["acc2"], Some(long_ago));

        assert_eq!(rotate_session_if_needed(&conn, "sess1").unwrap(), None, "a recovered account can run again, so there's nothing to rotate to");
        assert_eq!(session_account(&conn), "acc1");
        let status: String = conn.query_row("SELECT status FROM accounts WHERE id = 'acc1'", [], |r| r.get(0)).unwrap();
        assert_eq!(status, "healthy");
    }

    /// The stale-flag case this column was added for: an account benched before
    /// `exhausted_at` existed has no knowable age, so recovery treats it as
    /// expired and lifts it. Leaving it would bench the account forever — which
    /// is what happened, and what showed an out-of-quota banner on a terminal
    /// whose account was fine.
    #[test]
    fn a_benching_with_no_recorded_time_is_treated_as_expired() {
        let conn = seed(Some("auto"), &["acc2"], None);
        conn.execute("UPDATE accounts SET exhausted_at = NULL WHERE id = 'acc1'", []).unwrap();

        assert_eq!(recover_cooled_down_accounts(&conn).unwrap(), 1, "an undateable benching must be lifted, not kept");
        let (status, benched): (String, Option<String>) =
            conn.query_row("SELECT status, exhausted_at FROM accounts WHERE id = 'acc1'", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!(status, "healthy");
        assert_eq!(benched, None, "recovery must clear the timestamp too, or the next flag can't be dated");
    }

    /// The repoint bug: touching an account for an unrelated reason used to
    /// refresh `updated_at`, which recovery read as "benched just now" and so
    /// pushed the cooldown out by however long it had already been benched.
    #[test]
    fn an_unrelated_write_does_not_postpone_recovery() {
        let long_ago = (Utc::now() - Duration::hours(EXHAUSTION_COOLDOWN_HOURS + 1)).to_rfc3339();
        let conn = seed(Some("auto"), &["acc2"], Some(long_ago));

        // Exactly what `set_account_config_dir` does.
        conn.execute("UPDATE accounts SET config_dir_path = '/elsewhere', updated_at = ?1 WHERE id = 'acc1'", params![Utc::now().to_rfc3339()]).unwrap();

        assert_eq!(recover_cooled_down_accounts(&conn).unwrap(), 1, "the cooldown is measured from the benching, not from the last edit");
    }

    #[test]
    fn an_account_still_inside_its_cooldown_does_not_recover() {
        let recent = (Utc::now() - Duration::hours(EXHAUSTION_COOLDOWN_HOURS - 1)).to_rfc3339();
        let conn = seed(Some("auto"), &["acc2"], Some(recent));

        let switch = rotate_session_if_needed(&conn, "sess1").unwrap();
        assert!(switch.is_some(), "an account still rate-limited must be rotated away from, not reused");
        assert_eq!(session_account(&conn), "acc2");
    }

    /// Round-robin over a real pool: the account with no `rotation_events`
    /// row wins, and once it's been used the *other* one is next.
    #[test]
    fn round_robin_cycles_through_the_pool_across_successive_rotations() {
        let conn = seed(Some("round_robin"), &["acc2", "acc3"], None);
        conn.execute(
            "INSERT INTO rotation_events (id, project_id, from_account_id, to_account_id, reason, created_at)
             VALUES ('e1','proj1','acc1','acc2','test','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let first = rotate_session_if_needed(&conn, "sess1").unwrap().unwrap();
        assert_eq!(first.to_account_id, "acc3", "acc3 has never been rotated to, so it goes first");

        // Exhaust the new account and rotate again — acc2 (used long ago) is
        // now the least-recently-used of the remaining pool.
        // Both columns, exactly as `mark_account_exhausted` writes them: with
        // `exhausted_at` left NULL, recovery would read the benching as
        // undateable, lift it, and there'd be nothing to rotate away from.
        conn.execute("UPDATE accounts SET status = 'exhausted', exhausted_at = ?1, updated_at = ?1 WHERE id = 'acc3'", params![Utc::now().to_rfc3339()]).unwrap();
        let second = rotate_session_if_needed(&conn, "sess1").unwrap().unwrap();
        assert_eq!(second.to_account_id, "acc2");
    }

    // -- terminal rotation suggestion ---------------------------------------

    /// The pure suggestion logic: which allowed, healthy account a project
    /// should offer, given the exhausted one. Tested directly against
    /// `load_candidates` + `pick` (what the command wraps) without a mock app.
    fn suggest_for_project(conn: &Connection, project_id: &str, exhausted: &str, policy: Policy) -> Option<String> {
        let candidates = load_candidates(conn, project_id, "claude", exhausted).unwrap();
        let policy = if policy == Policy::Manual { Policy::Auto } else { policy };
        pick(policy, &candidates).map(|c| c.account_id.clone())
    }

    #[test]
    fn suggests_an_allowed_healthy_account_and_never_the_exhausted_one() {
        let conn = seed(Some("auto"), &["acc2", "acc3"], None);
        let picked = suggest_for_project(&conn, "proj1", "acc1", Policy::Auto).expect("a project with allowed alternatives must suggest one");
        assert!(picked == "acc2" || picked == "acc3");
        assert_ne!(picked, "acc1");
    }

    /// With no accounts checked for the project there's nothing to offer — the
    /// banner then warns without a reopen button.
    #[test]
    fn suggests_nothing_when_the_project_allows_no_other_account() {
        let conn = seed(Some("auto"), &[], None);
        assert_eq!(suggest_for_project(&conn, "proj1", "acc1", Policy::Auto), None);
    }

    /// A different provider's account is never a substitute even if allowed —
    /// its CLI and config dir are incompatible.
    #[test]
    fn never_suggests_a_different_providers_account() {
        let conn = seed(Some("auto"), &["other1"], None);
        assert_eq!(suggest_for_project(&conn, "proj1", "acc1", Policy::Auto), None);
    }

    /// In the terminal model, `Manual` still suggests (the user makes the call
    /// with the button) — it just isn't an *automatic* switch. So the fallback
    /// to `Auto` must produce a real candidate.
    #[test]
    fn manual_policy_still_yields_a_suggestion_for_a_terminal() {
        let conn = seed(Some("manual"), &["acc2"], None);
        assert_eq!(suggest_for_project(&conn, "proj1", "acc1", Policy::Manual), Some("acc2".to_string()));
    }

    /// `allowed_benched_accounts` is the poller's re-check list, so it must be
    /// exactly the mirror of `load_candidates`: allowed, same provider, *not*
    /// healthy, and never the account that's already benched and asking.
    #[test]
    fn benched_candidates_are_the_allowed_unusable_accounts_of_the_same_provider() {
        let conn = seed(Some("auto"), &["acc2", "acc3", "other1"], None);
        // acc2 is allowed and benched — the one worth re-checking. acc3 is
        // allowed but healthy (nothing to re-check). other1 is allowed and
        // benched but speaks a different CLI.
        conn.execute("UPDATE accounts SET status = 'exhausted', exhausted_at = ?1 WHERE id IN ('acc2','other1')", params![Utc::now().to_rfc3339()]).unwrap();

        let benched = allowed_benched_accounts(&conn, "proj1", "claude", "acc1").unwrap();
        assert_eq!(benched, vec!["acc2".to_string()]);
    }

    #[test]
    fn a_project_that_allows_nothing_has_no_benched_candidates() {
        let conn = seed(Some("auto"), &[], None);
        assert!(allowed_benched_accounts(&conn, "proj1", "claude", "acc1").unwrap().is_empty());
    }

    // -- the project's fallback account ------------------------------------

    /// Adds a healthy custom-endpoint account and makes it proj1's fallback.
    fn with_fallback(conn: &Connection) {
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at, base_url)
             VALUES ('fb1','custom','key','DeepSeek','personal','','','fb1','healthy','now','now','https://api.deepseek.com/anthropic')",
            [],
        )
        .unwrap();
        store_project_fallback(conn, "proj1", Some("fb1")).unwrap();
    }

    /// The contract in one test: the fallback is what's offered when nothing of
    /// the same provider is left, and the offer names where the code will go.
    #[test]
    fn the_fallback_is_offered_only_once_no_same_provider_account_is_left() {
        let conn = seed(Some("auto"), &[], None);
        assert_eq!(suggest_for_terminal(&conn, "proj1", "acc1").unwrap(), None, "no fallback configured: the banner is today's");

        with_fallback(&conn);
        let offered = suggest_for_terminal(&conn, "proj1", "acc1").unwrap().expect("with nothing else left the fallback is offered");
        assert_eq!((offered.account_id.as_str(), offered.fallback_host.as_deref()), ("fb1", Some("api.deepseek.com")));
    }

    #[test]
    fn a_healthy_account_of_the_same_provider_always_wins_over_the_fallback() {
        let conn = seed(Some("auto"), &["acc2"], None);
        with_fallback(&conn);
        let offered = suggest_for_terminal(&conn, "proj1", "acc1").unwrap().unwrap();
        assert_eq!((offered.account_id.as_str(), offered.fallback_host), ("acc2", None));
    }

    /// A fallback that can't run is not an offer, and neither is the account
    /// the terminal is already on.
    #[test]
    fn an_unusable_fallback_is_not_offered() {
        let conn = seed(Some("auto"), &[], None);
        with_fallback(&conn);
        assert_eq!(suggest_for_terminal(&conn, "proj1", "fb1").unwrap(), None, "already on it");
        conn.execute("UPDATE accounts SET status = 'exhausted' WHERE id = 'fb1'", []).unwrap();
        assert_eq!(suggest_for_terminal(&conn, "proj1", "acc1").unwrap(), None);
    }

    #[test]
    fn only_a_custom_endpoint_account_can_be_a_fallback() {
        let conn = seed(Some("auto"), &[], None);
        assert!(store_project_fallback(&conn, "proj1", Some("acc2")).is_err(), "another Claude account belongs in the rotation pool");
        assert!(store_project_fallback(&conn, "proj1", Some("other1")).is_err());
        assert!(store_project_fallback(&conn, "proj1", Some("nonesuch")).is_err());
        assert!(store_project_fallback(&conn, "nonesuch", None).is_err());
        with_fallback(&conn);
        assert_eq!(project_fallback(&conn, "proj1").unwrap().map(|f| f.account_id), Some("fb1".to_string()));
        store_project_fallback(&conn, "proj1", None).unwrap();
        assert_eq!(project_fallback(&conn, "proj1").unwrap(), None);
    }

    /// The switcher shows the fallback last, with its host, and never as the
    /// policy's pick. From the fallback itself it shows the Claude pool, which
    /// is the only way back.
    #[test]
    fn the_switcher_lists_the_fallback_last_and_the_way_back_from_it() {
        let conn = seed(Some("auto"), &["acc2", "acc3"], None);
        with_fallback(&conn);

        let rows = rotatable_accounts(&conn, "proj1", "claude", "acc1").unwrap();
        assert_eq!(rows.iter().map(|r| r.account_id.as_str()).collect::<Vec<_>>(), vec!["acc1", "acc2", "acc3", "fb1"]);
        let fallback = rows.last().unwrap();
        assert_eq!(fallback.fallback_host.as_deref(), Some("api.deepseek.com"));
        assert!(!fallback.suggested);
        assert!(rows[..3].iter().all(|r| r.fallback_host.is_none()));

        assert_eq!(switcher_provider(&conn, "proj1", "fb1").unwrap(), "claude");
        let back = rotatable_accounts(&conn, "proj1", "claude", "fb1").unwrap();
        assert_eq!(back.iter().map(|r| r.account_id.as_str()).collect::<Vec<_>>(), vec!["fb1", "acc2", "acc3"]);
        assert!(back[0].current && back[0].fallback_host.is_some());
        assert!(back.iter().any(|r| r.suggested), "a healthy Claude account is marked as the way back");
    }

    /// A Codex terminal is not offered a fallback that could not take it over.
    #[test]
    fn the_fallback_only_appears_for_the_provider_it_stands_in_for() {
        let conn = seed(Some("auto"), &["other1"], None);
        with_fallback(&conn);
        let rows = rotatable_accounts(&conn, "proj1", "codex", "other1").unwrap();
        assert_eq!(rows.iter().map(|r| r.account_id.as_str()).collect::<Vec<_>>(), vec!["other1"]);
    }

    #[test]
    fn a_switch_onto_the_fallback_says_so_in_the_ledger() {
        let conn = seed(Some("auto"), &[], None);
        with_fallback(&conn);
        record_terminal_rotation(&conn, "project", "proj1", "acc1", "fb1").unwrap();
        let reason: String = conn.query_row("SELECT reason FROM rotation_events WHERE to_account_id = 'fb1'", [], |r| r.get(0)).unwrap();
        assert!(reason.starts_with("exhausted") && reason.ends_with("→ fallback endpoint"), "got: {reason}");
    }

    #[test]
    fn the_host_is_what_is_shown_for_an_endpoint() {
        assert_eq!(endpoint_host("https://openrouter.ai/api"), "openrouter.ai");
        assert_eq!(endpoint_host("http://localhost:3001"), "localhost:3001");
        assert_eq!(endpoint_host("https://user@api.z.ai/api/anthropic"), "api.z.ai");
        assert_eq!(endpoint_host("not-a-url"), "not-a-url");
    }

    // -- the header chip's account switcher ---------------------------------

    fn switcher_ids(conn: &Connection, current: &str) -> Vec<String> {
        rotatable_accounts(conn, "proj1", "claude", current).unwrap().into_iter().map(|a| a.account_id).collect()
    }

    /// The pool is what the project checked in Settings → Rotation, and the
    /// account you're on leads it so the chip can say where you are.
    #[test]
    fn the_switcher_lists_the_projects_allowed_accounts_with_the_current_one_first() {
        let conn = seed(Some("auto"), &["acc2", "acc3"], None);
        assert_eq!(switcher_ids(&conn, "acc1"), vec!["acc1", "acc2", "acc3"]);
    }

    /// `acc1` isn't checked for the project, but it's the account the terminal
    /// is running under — dropping it would make the chip claim you're
    /// somewhere you aren't.
    #[test]
    fn the_current_account_is_listed_even_when_the_project_does_not_allow_it() {
        let conn = seed(Some("auto"), &["acc2"], None);
        let rows = rotatable_accounts(&conn, "proj1", "claude", "acc1").unwrap();
        assert_eq!(rows.iter().map(|a| a.account_id.as_str()).collect::<Vec<_>>(), vec!["acc1", "acc2"]);
        assert!(rows[0].current);
        assert!(!rows[1].current);
    }

    /// Unlike `load_candidates`, a benched account stays in the list — the UI
    /// disables it and says why. Hiding it would look like the account vanished
    /// from the pool the user configured.
    #[test]
    fn a_benched_account_is_listed_with_its_status_rather_than_hidden() {
        let conn = seed(Some("auto"), &["acc2", "acc3"], None);
        conn.execute("UPDATE accounts SET status = 'exhausted', exhausted_at = ?1 WHERE id = 'acc2'", params![Utc::now().to_rfc3339()]).unwrap();

        let rows = rotatable_accounts(&conn, "proj1", "claude", "acc1").unwrap();
        let acc2 = rows.iter().find(|a| a.account_id == "acc2").expect("a benched account must still be offered, greyed out");
        assert_eq!(acc2.status, "exhausted");
        assert!(!acc2.suggested, "a benched account is never the recommendation");
    }

    /// Switching CLI isn't switching account: a codex account can't run this
    /// terminal's command line, so it never appears even if it's allowed.
    #[test]
    fn the_switcher_never_offers_a_different_providers_account() {
        let conn = seed(Some("auto"), &["acc2", "other1"], None);
        assert_eq!(switcher_ids(&conn, "acc1"), vec!["acc1", "acc2"]);
    }

    /// The recommendation follows the project's policy: round-robin points at
    /// the account that went longest without being used, not the cheapest.
    #[test]
    fn the_suggested_row_follows_the_projects_rotation_policy() {
        let conn = seed(Some("round_robin"), &["acc2", "acc3"], None);
        conn.execute(
            "INSERT INTO rotation_events (id, project_id, from_account_id, to_account_id, reason, created_at)
             VALUES ('e1','proj1','acc1','acc2','test','2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        let rows = rotatable_accounts(&conn, "proj1", "claude", "acc1").unwrap();
        let suggested: Vec<&str> = rows.iter().filter(|a| a.suggested).map(|a| a.account_id.as_str()).collect();
        assert_eq!(suggested, vec!["acc3"], "acc3 has never been rotated to");
    }

    /// A `manual` project still gets a recommendation — clicking the row *is*
    /// the manual decision. Same fallback the out-of-quota banner uses.
    #[test]
    fn a_manual_project_still_marks_a_suggestion() {
        let conn = seed(Some("manual"), &["acc2"], None);
        let rows = rotatable_accounts(&conn, "proj1", "claude", "acc1").unwrap();
        assert!(rows.iter().any(|a| a.account_id == "acc2" && a.suggested));
    }

    /// Nothing to switch to: the list is just the account you're on, which is
    /// what tells the UI to point at Settings → Rotation instead.
    #[test]
    fn a_project_with_no_allowed_accounts_offers_only_the_current_one() {
        let conn = seed(Some("auto"), &[], None);
        assert_eq!(switcher_ids(&conn, "acc1"), vec!["acc1"]);
    }

    /// The ledger has to distinguish "I chose to move" from "I was pushed":
    /// both go through the same reopen, so the reason comes from whether the
    /// account left behind could still run.
    #[test]
    fn the_rotation_reason_separates_a_deliberate_switch_from_a_quota_one() {
        let conn = seed(Some("auto"), &["acc2"], None);
        assert!(terminal_rotation_reason(&conn, "acc1").starts_with("exhausted"));
        assert_eq!(terminal_rotation_reason(&conn, "acc2"), "manual (switched from the panel)");
    }

    #[test]
    fn project_of_terminal_resolves_both_kinds() {
        // `seed` already inserts a `sess1`; this test only needs the project
        // and that pre-existing session to resolve a session-kind target.
        let conn = seed(Some("auto"), &["acc2"], None);
        assert_eq!(project_of_terminal(&conn, "project", "proj1").unwrap(), "proj1");
        assert_eq!(project_of_terminal(&conn, "session", "sess1").unwrap(), "proj1");
        assert!(project_of_terminal(&conn, "bogus", "x").is_err());
    }
}
