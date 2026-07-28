//! Real interactive shells running inside Hangar, one PTY per terminal.
//!
//! Hangar's agents work in isolated worktrees (`.hangar/worktrees/<slug>`), and
//! until now the only way to look inside one was `open_path_in_terminal`
//! (commands/git_panel.rs), which hands off to Terminal.app and leaves the
//! app entirely. These terminals open *in* the worktree, next to the agent
//! that's editing it.
//!
//! Two things here differ deliberately from the rest of the codebase:
//!
//! 1. **Output goes over a `tauri::ipc::Channel`, not the event system.**
//!    Tauri's own docs say the event system "is not designed for low latency
//!    or high throughput situations", and that with async listeners "events
//!    may be processed out of order". Everywhere else in Hangar that's fine —
//!    the traffic is discrete session blocks. For a byte stream driving a
//!    terminal emulator, out-of-order delivery isn't degradation, it's a
//!    corrupted screen.
//!
//! 2. **Chunks are base64.** PTY output is raw bytes and a single `read()`
//!    can land in the middle of a multi-byte UTF-8 sequence, which
//!    `from_utf8_lossy` would replace with U+FFFD — permanently mangling any
//!    non-ASCII output. Base64 keeps the bytes intact all the way to
//!    xterm.js, which accepts a `Uint8Array` directly.

use crate::db::DbConnection;
use base64::Engine;
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use uuid::Uuid;

/// How much output to keep per terminal so the screen can be repainted when
/// the user navigates away and back. The PTY keeps running while nobody is
/// looking, so without this a `cargo build` started before switching views
/// would come back to a blank pane.
const SCROLLBACK_LIMIT_BYTES: usize = 256 * 1024;

/// Read buffer size. Also the granularity at which scrollback is trimmed —
/// see `push_scrollback`.
const READ_CHUNK_BYTES: usize = 8 * 1024;

/// How long to wait for a new shell to print its first prompt before typing
/// an init command anyway. Generous because a login shell sourcing a heavy
/// profile (nvm, rbenv, a slow prompt) can take a moment, and the cost of
/// giving up early is only the cosmetic double-echo this avoids.
const SHELL_READY_TIMEOUT: Duration = Duration::from_secs(5);

/// Grace period after the first output before typing. The prompt is printed
/// slightly before the shell's line editor is ready to receive input.
const SHELL_SETTLE: Duration = Duration::from_millis(120);

/// What a terminal was opened against. A session resolves to its worktree
/// when it has one, which is the whole point of the feature; a project
/// resolves to its own path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Project,
    Session,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalTarget {
    pub kind: TargetKind,
    pub id: String,
}

/// What to type into the shell as soon as it comes up, when a terminal is
/// opened against an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InitCommand {
    /// Launch the provider's CLI interactively, on a fresh conversation whose
    /// id Hangar owns (`claude --session-id <uuid>`). Owning the id is what
    /// lets Hangar later find and move that exact transcript — see the
    /// conversation-continuity plan.
    Start,
    /// Re-attach the CLI to a conversation Hangar already knows the id of
    /// (`claude --resume <uuid>`). Requires a `session_uuid`.
    Resume,
    /// Re-authenticate the account (`claude auth login --claudeai`). This is
    /// the only path in Hangar that can recover an account whose OAuth token
    /// expired — everything else assumes an account stays logged in for life,
    /// so the alternative is deleting it and adding it again.
    Login,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    pub id: String,
    /// What the pane header shows — the project or session name.
    pub title: String,
    pub cwd: String,
    pub kind: TargetKind,
    /// The project or session id this was opened against, so the frontend can
    /// tell "a terminal for this session already exists" without re-resolving
    /// paths.
    pub source_id: String,
    /// The account whose environment this shell is running under, if any. A
    /// shell carrying credentials has to say so in its header rather than
    /// looking identical to a plain one.
    pub account_name: Option<String>,
    /// The account id behind `account_name` — the frontend maps a terminal to
    /// its account (for the "this account is out of quota" banner) by this,
    /// and the rotation poller learns which accounts are in use from it.
    pub account_id: Option<String>,
    /// The Claude Code session id Hangar started this terminal on
    /// (`claude --session-id <uuid>`), when it's an account-scoped claude
    /// terminal. This is what makes the conversation addressable — the
    /// transcript lives at `<config_dir>/projects/<slug>/<session_uuid>.jsonl`
    /// — so it can be resumed or copied to another account later. `None` for a
    /// plain shell, a login, or a non-claude terminal.
    pub session_uuid: Option<String>,
    /// A name the user typed for this panel, which wins over `title` in the UI.
    /// `None` until they rename it.
    pub display_name: Option<String>,
}

/// What a terminal's agent is doing, as far as Hangar can tell from the
/// outside.
///
/// This is the thing the app was missing: with six terminals open, knowing
/// which one needs you meant reading six panes. Every comparable tool has some
/// form of it (Orca's state dots, herdr's "status detection"), and it's the
/// foundation for notifying, sorting and summarising — none of which can exist
/// without a state to observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TerminalState {
    /// Producing output right now.
    Working,
    /// Quiet after having worked — the agent has almost certainly finished its
    /// turn and is waiting for you.
    Waiting,
    /// Nothing has happened yet, or the shell is just sitting at a prompt.
    Idle,
    /// The process is gone.
    Exited,
}

/// How long a terminal must stay quiet before "working" becomes "waiting".
///
/// Deliberately generous. An agent thinking between tool calls can go quiet for
/// several seconds, and a state that flickers back and forth is worse than no
/// state at all — it trains you to ignore the dot.
const QUIET_BEFORE_WAITING: Duration = Duration::from_secs(4);

/// Terminal-title escape sequences: `ESC ] 0 ; text BEL` and the `2` variant,
/// either terminated by BEL (0x07) or ST (`ESC \`).
///
/// Parsed straight out of the PTY stream rather than asked for: it needs no
/// hook, no configuration of the user's Claude Code, and works for any CLI that
/// sets a title. Claude Code's status line is the richest source of this, but
/// nothing here depends on it being present.
fn parse_osc_title(chunk: &[u8]) -> Option<String> {
    let mut i = 0;
    let mut found = None;
    while i + 3 < chunk.len() {
        if chunk[i] == 0x1b && chunk[i + 1] == b']' && (chunk[i + 2] == b'0' || chunk[i + 2] == b'2') && chunk[i + 3] == b';' {
            let start = i + 4;
            let mut end = start;
            while end < chunk.len() && chunk[end] != 0x07 && !(chunk[end] == 0x1b && end + 1 < chunk.len() && chunk[end + 1] == b'\\') {
                end += 1;
            }
            if end < chunk.len() {
                // Later titles win: one read can carry several updates, and the
                // last one is the current state of the world.
                found = Some(String::from_utf8_lossy(&chunk[start..end]).trim().to_string());
                i = end;
            }
        }
        i += 1;
    }
    found.filter(|t| !t.is_empty())
}

/// One chunk of terminal output on its way to the frontend.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalChunk {
    /// Base64 of the raw PTY bytes — see the module docs for why this isn't a
    /// plain `String`.
    pub base64: String,
}

struct TerminalHandle {
    /// Kept alive purely so `resize` has something to call: dropping the
    /// master closes the PTY out from under the child.
    master: Box<dyn MasterPty + Send>,
    /// Shared so the deferred init-command thread can type into the shell
    /// after it's ready (see `open_terminal`), not only the keystroke command.
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    /// The channel of whoever is currently displaying this terminal, or
    /// `None` while nobody is. The reader thread keeps filling `scrollback`
    /// either way, so a background terminal never loses output.
    sink: Arc<Mutex<Option<Channel<TerminalChunk>>>>,
    /// Live state, written by the reader thread and by the watcher that ages
    /// `Working` into `Waiting`. Shared rather than stored on `info` because
    /// both of those run outside any command.
    activity: Arc<Mutex<Activity>>,
    info: TerminalInfo,
}

/// The observable part of a terminal: when it last spoke, what it called
/// itself, and what that adds up to.
#[derive(Debug, Clone)]
struct Activity {
    state: TerminalState,
    last_output: Instant,
    /// The last title the program set, when it sets one — Claude Code's status
    /// line puts the current task here, which is a far better label for a panel
    /// than the project name.
    title: Option<String>,
}

impl Activity {
    fn new() -> Self {
        Activity { state: TerminalState::Idle, last_output: Instant::now(), title: None }
    }
}

pub struct TerminalRegistry(pub Mutex<HashMap<String, TerminalHandle>>);

impl TerminalRegistry {
    pub fn new() -> Self {
        TerminalRegistry(Mutex::new(HashMap::new()))
    }
}

/// Ensures the persisted terminals are restored exactly once per app run. The
/// frontend calls `restore_terminals` on every mount of the terminals view;
/// only the first does the restore, later mounts just get the live list.
pub struct RestoreGuard(pub AtomicBool);

impl RestoreGuard {
    pub fn new() -> Self {
        RestoreGuard(AtomicBool::new(false))
    }
}

/// Appends a chunk to the ring buffer, dropping whole chunks off the front
/// once over the cap.
///
/// Trimming a terminal byte stream can cut an ANSI escape sequence in half,
/// which would paint garbage on replay. Dropping from the front in whole
/// `read()` chunks rather than byte-by-byte doesn't make that impossible, but
/// it keeps the seam at a boundary the shell actually wrote, which in
/// practice lands between complete sequences. The alternative — parsing
/// escape sequences to find a safe cut point — is a terminal emulator's job,
/// not a scrollback buffer's.
fn push_scrollback(buffer: &mut VecDeque<u8>, chunk: &[u8]) {
    buffer.extend(chunk.iter().copied());
    while buffer.len() > SCROLLBACK_LIMIT_BYTES {
        let excess = buffer.len() - SCROLLBACK_LIMIT_BYTES;
        let drop_n = excess.max(READ_CHUNK_BYTES).min(buffer.len());
        buffer.drain(..drop_n);
    }
}

/// The shell to launch. `$SHELL` is what the user actually chose; the
/// fallback only matters for a process started without it.
fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// Environment variables that decide *which account* a provider CLI acts as.
/// Inherited from Hangar's own process (a developer running
/// `ANTHROPIC_API_KEY=… npm run tauri dev`, or a `CLAUDE_CONFIG_DIR` exported
/// in a shell profile) any of these would silently override the account the
/// user picked, so an account-scoped terminal starts without them.
/// `login_manager::auth_status_for_dir` strips `ANTHROPIC_API_KEY` for exactly
/// this reason — "never let an inherited ANTHROPIC_API_KEY mask the real login
/// state" — and the hazard is the same for a shell the user will run `claude`
/// in by hand.
const IDENTITY_ENV_VARS: &[&str] = &["CLAUDE_CONFIG_DIR", "ANTHROPIC_API_KEY"];

/// Full parent environment plus a sane `TERM`, and — for an account-scoped
/// terminal — with the inherited identity variables removed and the account's
/// own added.
///
/// Inheriting the parent environment matters more than usual here: an app
/// launched from Finder has a minimal PATH (the same problem
/// `providers::resolve_binary` exists to work around), and a login shell is
/// what reloads the user's real profile.
fn terminal_env(account_vars: &[(String, String)], scoped_to_account: bool) -> Vec<(String, String)> {
    merge_env(std::env::vars().collect(), account_vars, scoped_to_account)
}

/// The merge itself, split out from `terminal_env` so the stripping rules can
/// be tested against a hand-built parent environment instead of whatever the
/// test runner's process happens to have set.
///
/// `scoped_to_account` is a separate flag rather than being inferred from
/// `account_vars` being non-empty: a subscription account contributes no
/// variables at all now that its config dir travels on the command line, and
/// it still must not inherit a stray identity from the parent.
fn merge_env(base: Vec<(String, String)>, account_vars: &[(String, String)], scoped_to_account: bool) -> Vec<(String, String)> {
    let mut env = base;
    if scoped_to_account {
        env.retain(|(k, _)| !IDENTITY_ENV_VARS.contains(&k.as_str()));
        env.extend(account_vars.iter().cloned());
    }
    if !env.iter().any(|(k, _)| k == "TERM") {
        env.push(("TERM".to_string(), "xterm-256color".to_string()));
    }
    // Real terminal emulators set this; a GUI app's inherited environment does
    // not. Without it a CLI that checks for truecolour falls back to 256, which
    // is a visible downgrade for anything that draws its own interface.
    if !env.iter().any(|(k, _)| k == "COLORTERM") {
        env.push(("COLORTERM".to_string(), "truecolor".to_string()));
    }
    env
}

/// The account fields both the command builder and `open_terminal` need.
pub(crate) struct AccountForTerminal {
    pub provider: String,
    pub billing: String,
    pub config_dir_path: String,
    pub name: String,
}

pub(crate) fn load_account(conn: &rusqlite::Connection, account_id: &str) -> Result<AccountForTerminal, String> {
    conn.query_row(
        "SELECT provider, billing, config_dir_path, name FROM accounts WHERE id = ?1",
        rusqlite::params![account_id],
        |row| {
            Ok(AccountForTerminal {
                provider: row.get(0)?,
                billing: row.get(1)?,
                config_dir_path: row.get(2)?,
                name: row.get(3)?,
            })
        },
    )
    .map_err(|e| format!("account lookup failed: {e}"))
}

/// The API key, for a `key`-billed account, as process environment.
///
/// Deliberately *not* part of the command line: `CLAUDE_CONFIG_DIR` is a path
/// and belongs in the visible command, but a shell records every line it runs
/// in `~/.zsh_history`, so an inline `ANTHROPIC_API_KEY=sk-...` would write the
/// real key to disk in plaintext and leave it in the pane's scrollback. As
/// ambient environment it still reaches the CLI, and the displayed command
/// stays exactly the command that runs.
fn account_key_env(account: &AccountForTerminal, account_id: &str) -> Result<Vec<(String, String)>, String> {
    if account.billing != "key" {
        return Ok(Vec::new());
    }
    let key = crate::credential_vault::get(account_id)?;
    // Which variable carries the key is the provider's business: Anthropic
    // reads `ANTHROPIC_API_KEY`, Moonshot is reached through Claude Code's
    // `ANTHROPIC_AUTH_TOKEN`, and Codex wants `OPENAI_API_KEY`.
    let var = crate::providers::spec(&account.provider).map(|s| s.key_env).unwrap_or("ANTHROPIC_API_KEY");
    let mut env = vec![(var.to_string(), key)];
    for (k, v) in crate::providers::spec(&account.provider).map(|s| s.extra_env).unwrap_or(&[]) {
        env.push((k.to_string(), v.to_string()));
    }
    Ok(env)
}

/// Wraps a value in single quotes for the shell, escaping any single quotes
/// inside it — but only when it actually needs it. A bare
/// `CLAUDE_CONFIG_DIR=/Users/me/.claude-work claude` is what a person types by
/// hand, and this string is shown to the user as *the* command; quoting it
/// unconditionally would make the display noisier than the reality.
fn shell_quote(value: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "._-/=:@+,".contains(c);
    if !value.is_empty() && value.chars().all(safe) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// The exact command a terminal for this account will run — inline
/// `CLAUDE_CONFIG_DIR=<dir>` prefix and all, the same shape used by hand:
///
/// ```text
/// CLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude
/// ```
///
/// The inline prefix rather than an exported variable is the point: the
/// command is shown on the account row before opening, typed verbatim into
/// the shell, and left in the scrollback afterwards — so what the user was
/// promised, what ran, and what they can copy back out are all one string.
/// Scoping the variable to this one command also leaves the rest of the shell
/// neutral instead of silently reinterpreting every later `claude` the user
/// types.
///
/// The binary is the **bare name**, not `providers::resolve_binary`'s absolute
/// path. The shell is started with `-l`, so it loads the user's profile and
/// therefore the PATH that puts `claude` within reach — which is exactly why
/// `CLAUDE_CONFIG_DIR=… claude` is what a person types by hand, and this
/// string has to be that. `is_provider_available` still gates below, so a
/// genuinely missing CLI is caught before a terminal ever opens; if some
/// unusual profile ever left it off the interactive PATH, the fix is to fall
/// back to the absolute path only when the bare name doesn't resolve.
///
/// Errors if the provider's CLI isn't installed, so that surfaces as a
/// message in the UI rather than a terminal that opens and immediately says
/// `command not found`.
/// `prompt` (only with `InitCommand::Start`) is a first message handed to
/// Claude Code as a positional argument: `claude '<prompt>'` starts the same
/// interactive session, but already working on that message. It's how "Send
/// PR comments to a terminal" gets the review feedback in front of the agent.
/// A prompt with newlines would trip zsh's PS2 continuation prompt inside the
/// quotes, so callers pass a single-line prompt.
pub(crate) fn account_command_line(account: &AccountForTerminal, init: InitCommand, session_uuid: Option<&str>, prompt: Option<&str>) -> Result<String, String> {
    let spec = crate::providers::spec(&account.provider).ok_or_else(|| format!("unknown provider '{}'", account.provider))?;
    let binary = spec.binary;
    if !crate::providers::is_provider_available(&account.provider) {
        return Err(format!("the `{binary}` CLI isn't installed on this machine"));
    }
    let config = shell_quote(&account.config_dir_path);
    // Fixed environment the provider always needs — for Kimi, pointing Claude
    // Code at Moonshot's Anthropic-compatible endpoint. Rendered into the
    // command line so what the picker previews is literally what runs.
    let extra: String = spec.extra_env.iter().map(|(k, v)| format!("{k}={} ", shell_quote(v))).collect();
    // The trailing prompt, if any — a positional first message. Shared by Start
    // and Resume (both open an interactive session that can carry one).
    let prompt_arg = || match prompt.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => format!(" {}", shell_quote(p)),
        None => String::new(),
    };
    let args = match init {
        InitCommand::Start => match (spec.session, session_uuid) {
            // Hangar owns the id so it can locate this exact transcript later.
            (crate::providers::SessionControl::MintableId { start_flag, .. }, Some(uuid)) => {
                format!(" {start_flag} {}{}", shell_quote(uuid), prompt_arg())
            }
            // Codex assigns its own id and has no flag to name one — passing
            // Claude's would be `error: unexpected argument '--session-id'`,
            // which is exactly what a terminal used to open with.
            _ => prompt_arg(),
        },
        InitCommand::Resume => match spec.session {
            crate::providers::SessionControl::MintableId { resume_flag, .. } => {
                let uuid = session_uuid.ok_or("resuming a terminal needs the session id to resume")?;
                format!(" {resume_flag} {}{}", shell_quote(uuid), prompt_arg())
            }
            // We never held an id for this CLI, so there's nothing to resume by
            // name. Starting fresh is the honest outcome; resuming "the most
            // recent session" would be a guess dressed as continuity.
            _ => prompt_arg(),
        },
        // A login command takes no prompt or session — there's nothing to seed.
        InitCommand::Login => match spec.login_args {
            Some(login) => format!(" {}", login.join(" ")),
            None => return Err(format!("'{}' authenticates with an API key — there's no login to run", account.provider)),
        },
    };
    Ok(format!("{}={config} {extra}{binary}{args}", spec.config_dir_env))
}

/// The full script a terminal runs: change directory first, then the account's
/// command, each on its own line.
///
/// The `cd` is deliberately typed rather than left to `CommandBuilder::cwd`.
/// It's the honest thing to show — the working directory is half of what makes
/// this terminal different from any other — and it's a real safeguard: the
/// shell starts with `-l`, and a profile that changes directory would
/// otherwise leave the CLI running somewhere the user never chose. Setting
/// `cwd` on the spawn as well makes this redundant in the normal case, which
/// is the point.
pub(crate) fn terminal_script(cwd: &str, account: &AccountForTerminal, init: InitCommand, session_uuid: Option<&str>, prompt: Option<&str>) -> Result<String, String> {
    Ok(format!("cd {}\n{}", shell_quote(cwd), account_command_line(account, init, session_uuid, prompt)?))
}

/// Resolves a target to the directory its terminal should start in, plus the
/// title to show. A session with a worktree resolves to the worktree — that
/// is the case this whole feature exists for.
/// `resolve_target` for callers outside this module — the Files screen resolves
/// a project or a session to a working directory by exactly the same rules a
/// terminal does, and two implementations of "where does this target live" would
/// drift apart the first time a worktree moved.
pub(crate) fn resolve_target_for_files(conn: &rusqlite::Connection, target: &TerminalTarget) -> Result<(String, String), String> {
    resolve_target(conn, target)
}

fn resolve_target(conn: &rusqlite::Connection, target: &TerminalTarget) -> Result<(String, String), String> {
    match target.kind {
        TargetKind::Project => conn
            .query_row(
                "SELECT path, name FROM projects WHERE id = ?1",
                rusqlite::params![target.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|e| format!("project lookup failed: {e}")),
        TargetKind::Session => {
            let (name, worktree_path, project_id): (String, Option<String>, String) = conn
                .query_row(
                    "SELECT name, worktree_path, project_id FROM sessions WHERE id = ?1",
                    rusqlite::params![target.id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(|e| format!("session lookup failed: {e}"))?;
            let cwd = match worktree_path {
                Some(path) => path,
                None => conn
                    .query_row("SELECT path FROM projects WHERE id = ?1", rusqlite::params![project_id], |row| row.get(0))
                    .map_err(|e| format!("project lookup failed: {e}"))?,
            };
            Ok((cwd, name))
        }
    }
}

#[tauri::command]
pub fn open_terminal<R: Runtime>(
    app: AppHandle<R>,
    target: TerminalTarget,
    account_id: Option<String>,
    init: Option<InitCommand>,
    session_uuid: Option<String>,
    init_prompt: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<TerminalInfo, String> {
    let (cwd, title, account) = {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let (cwd, title) = resolve_target(&conn, &target)?;
        let account = match &account_id {
            Some(id) => Some(load_account(&conn, id)?),
            None => None,
        };
        (cwd, title, account)
    };
    if !std::path::Path::new(&cwd).is_dir() {
        return Err(format!("{cwd} no longer exists on disk — the worktree may have been removed"));
    }

    // Hangar owns the conversation id for an account-scoped `Start`: use the one
    // the caller supplied (a restore/transfer resuming a known id) or mint a
    // fresh one. `Resume` must carry its id; `Login` and plain shells have none.
    let mintable = account
        .as_ref()
        .and_then(|a| crate::providers::spec(&a.provider))
        .is_some_and(|s| matches!(s.session, crate::providers::SessionControl::MintableId { .. }));
    let session_uuid = match (&account, init) {
        // Only for a CLI that lets Hangar choose the id. Minting one for Codex
        // would record a conversation id nothing can address — and every
        // feature that reads `session_uuid` (the switcher's "this carries your
        // conversation", restore's `--resume`) would be promising on it.
        (Some(_), Some(InitCommand::Start)) if mintable => Some(session_uuid.unwrap_or_else(|| Uuid::new_v4().to_string())),
        (Some(_), Some(InitCommand::Resume)) if mintable => Some(session_uuid.ok_or("resuming a terminal needs a session id")?),
        _ => None,
    };

    // Resolve the command before spawning anything: a missing CLI should
    // surface as an error the user can act on, not as a terminal that opens
    // and greets them with `command not found`.
    let init_line = match (&account, init) {
        (Some(account), Some(init)) => Some(format!("{}\n", terminal_script(&cwd, account, init, session_uuid.as_deref(), init_prompt.as_deref())?)),
        // A command only means something for an account — "start claude" as
        // nobody in particular isn't a thing Hangar offers.
        (None, Some(_)) => return Err("choose an account before picking a command to run".to_string()),
        _ => None,
    };
    let mut account_vars = match (&account, &account_id) {
        (Some(account), Some(id)) => account_key_env(account, id)?,
        _ => Vec::new(),
    };
    // The project's git identity and push credential go into the PTY too, not
    // just into Hangar's own git calls: the process that runs `git push` most
    // often here is the agent, in a shell whose environment can't be changed
    // once it's live.
    {
        let db = app.state::<DbConnection>();
        let project_id = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            let kind = match target.kind {
                TargetKind::Project => "project",
                TargetKind::Session => "session",
            };
            crate::rotation::project_of_terminal(&conn, kind, &target.id).ok()
        };
        if let Some(project_id) = project_id {
            account_vars.extend(crate::git_identity::git_env_for_project(&app, &project_id));
        }
    }
    let account_name = account.as_ref().map(|a| a.name.clone());

    let pty = native_pty_system();
    let pair = pty
        // A zero dimension makes some shells misbehave before the frontend's
        // first real `fit()` lands, so clamp to something sane.
        .openpty(PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(user_shell());
    cmd.arg("-l");
    cmd.cwd(&cwd);
    for (k, v) in terminal_env(&account_vars, account.is_some()) {
        cmd.env(k, v);
    }

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(pair.master.take_writer().map_err(|e| e.to_string())?));

    let id = Uuid::new_v4().to_string();
    let info = TerminalInfo {
        id: id.clone(),
        title,
        cwd: cwd.clone(),
        kind: target.kind,
        source_id: target.id.clone(),
        account_name,
        account_id: account_id.clone(),
        session_uuid,
        // A fresh terminal has no name of its own; `restore_terminals` puts back
        // the one the user typed before, via `rename_terminal`.
        display_name: None,
    };

    let scrollback = Arc::new(Mutex::new(VecDeque::new()));
    let sink: Arc<Mutex<Option<Channel<TerminalChunk>>>> = Arc::new(Mutex::new(None));
    let activity = Arc::new(Mutex::new(Activity::new()));

    {
        let scrollback = scrollback.clone();
        let sink = sink.clone();
        let activity = activity.clone();
        let id = id.clone();
        let app_for_reader = app.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; READ_CHUNK_BYTES];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        if let Ok(mut buffer) = scrollback.lock() {
                            push_scrollback(&mut buffer, chunk);
                        }
                        // Output means the agent is doing something. The title,
                        // when there is one, comes from the same bytes — no
                        // extra channel, no hook, no configuration.
                        if let Ok(mut act) = activity.lock() {
                            act.last_output = Instant::now();
                            if let Some(title) = parse_osc_title(chunk) {
                                act.title = Some(title);
                            }
                            if act.state != TerminalState::Working {
                                act.state = TerminalState::Working;
                                emit_state(&app_for_reader, &id, TerminalState::Working, act.title.clone());
                            }
                        }
                        if let Ok(sink) = sink.lock() {
                            if let Some(channel) = sink.as_ref() {
                                let payload = TerminalChunk { base64: base64::engine::general_purpose::STANDARD.encode(chunk) };
                                let _ = channel.send(payload);
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            if let Ok(mut act) = activity.lock() {
                act.state = TerminalState::Exited;
            }
            emit_state(&app_for_reader, &id, TerminalState::Exited, None);
            log::info!("terminal {id}: pty closed");
        });
    }

    // Type the init command only once the shell is actually at its prompt.
    //
    // Writing it straight after spawn does work — the tty's line discipline
    // buffers input, so the shell reads it whenever it gets there — but it
    // looks broken: the tty echoes the raw bytes immediately, then the shell
    // redraws the same line after its prompt, so the user sees every command
    // twice before anything runs. Waiting for the shell's first output means
    // it's typed once and echoed once.
    //
    // Off the calling thread, because this is a synchronous command and
    // blocking it would freeze the IPC dispatch thread (see `crate::blocking`).
    if let Some(line) = init_line {
        let scrollback = scrollback.clone();
        let writer = writer.clone();
        let id = id.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + SHELL_READY_TIMEOUT;
            // Any output at all means the shell has started and printed its
            // prompt. A shell configured to print nothing would just fall
            // through to the deadline and still get its command.
            while Instant::now() < deadline {
                if scrollback.lock().map(|b| !b.is_empty()).unwrap_or(true) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            // The prompt is written before the shell finishes wiring up its
            // line editor; typing into that gap gets echoed but not edited,
            // which looks the same as the bug this avoids.
            std::thread::sleep(SHELL_SETTLE);

            let Ok(mut writer) = writer.lock() else { return };
            if let Err(e) = writer.write_all(line.as_bytes()).and_then(|()| writer.flush()) {
                log::warn!("terminal {id}: could not type the init command: {e}");
            }
        });
    }

    app.state::<TerminalRegistry>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id.clone(), TerminalHandle { master: pair.master, writer, child, scrollback, sink, activity, info: info.clone() });

    // Mirror the terminal to disk so it can be reopened next launch. Only a
    // registered terminal is persisted, so this can't outlive the shell.
    persist_terminal(&app, &info);

    log::info!("terminal {id} opened in {cwd}");

    // The pane is about to decide whether to show an out-of-quota banner off
    // this account's stored status, so verify it now rather than letting the
    // 5-minute poller get to it whenever.
    if let Some(account_id) = account_id {
        spawn_account_check(&app, account_id);
    }

    Ok(info)
}

/// Records an open terminal in `open_terminals` so a restart can reopen it.
/// Best-effort: a persistence miss costs a terminal that won't come back next
/// launch, never a broken open.
fn persist_terminal<R: Runtime>(app: &AppHandle<R>, info: &TerminalInfo) {
    let Some(db) = app.try_state::<DbConnection>() else { return };
    let Ok(conn) = db.0.lock() else { return };
    let kind = match info.kind {
        TargetKind::Project => "project",
        TargetKind::Session => "session",
    };
    if let Err(e) = conn.execute(
        "INSERT OR REPLACE INTO open_terminals (id, target_kind, source_id, account_id, session_uuid, display_name, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![info.id, kind, info.source_id, info.account_id, info.session_uuid, info.display_name, chrono::Utc::now().to_rfc3339()],
    ) {
        log::warn!("persist_terminal: {e}");
    }
}

/// Drops a terminal's persisted row — the user closed it, so it must not come
/// back on the next launch. Called by `close_terminal`, never by `kill_all`.
fn forget_terminal<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let Some(db) = app.try_state::<DbConnection>() else { return };
    let Ok(conn) = db.0.lock() else { return };
    let _ = conn.execute("DELETE FROM open_terminals WHERE id = ?1", rusqlite::params![id]);
}

/// Points a terminal's output at `channel` and returns everything it has
/// printed so far, base64-encoded.
///
/// Both halves happen under one lock on purpose: fetching the scrollback and
/// then installing the channel as two separate steps would drop any bytes
/// that arrived in between, which is exactly the moment a command finishes
/// and prints its result.
#[tauri::command]
pub fn attach_terminal<R: Runtime>(app: AppHandle<R>, id: String, channel: Channel<TerminalChunk>) -> Result<String, String> {
    let registry = app.state::<TerminalRegistry>();
    let map = registry.0.lock().map_err(|e| e.to_string())?;
    let handle = map.get(&id).ok_or_else(|| format!("no terminal with id {id}"))?;

    let mut sink = handle.sink.lock().map_err(|e| e.to_string())?;
    let buffer = handle.scrollback.lock().map_err(|e| e.to_string())?;
    let snapshot: Vec<u8> = buffer.iter().copied().collect();
    *sink = Some(channel);

    Ok(base64::engine::general_purpose::STANDARD.encode(snapshot))
}

/// Stops streaming to whatever channel is attached, without killing the
/// shell — what a pane does when it unmounts. The PTY keeps running and
/// filling its scrollback.
#[tauri::command]
pub fn detach_terminal<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    let registry = app.state::<TerminalRegistry>();
    let map = registry.0.lock().map_err(|e| e.to_string())?;
    let Some(handle) = map.get(&id) else { return Ok(()) };
    let mut sink = handle.sink.lock().map_err(|e| e.to_string())?;
    *sink = None;
    Ok(())
}

#[tauri::command]
pub fn write_terminal<R: Runtime>(app: AppHandle<R>, id: String, data: String) -> Result<(), String> {
    let registry = app.state::<TerminalRegistry>();
    let mut map = registry.0.lock().map_err(|e| e.to_string())?;
    let handle = map.get_mut(&id).ok_or_else(|| format!("no terminal with id {id}"))?;
    let mut writer = handle.writer.lock().map_err(|e| e.to_string())?;
    writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Tells the PTY its real size. Without this the shell keeps its initial
/// dimensions and anything full-screen (`vim`, `htop`, a progress bar) draws
/// against the wrong width.
#[tauri::command]
pub fn resize_terminal<R: Runtime>(app: AppHandle<R>, id: String, cols: u16, rows: u16) -> Result<(), String> {
    let registry = app.state::<TerminalRegistry>();
    let map = registry.0.lock().map_err(|e| e.to_string())?;
    let handle = map.get(&id).ok_or_else(|| format!("no terminal with id {id}"))?;
    handle
        .master
        .resize(PtySize { rows: rows.max(1), cols: cols.max(1), pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn close_terminal<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    {
        let registry = app.state::<TerminalRegistry>();
        let mut map = registry.0.lock().map_err(|e| e.to_string())?;
        if let Some(mut handle) = map.remove(&id) {
            log::info!("terminal {id} closed");
            let _ = handle.child.kill();
        }
    }
    // The user closed this one on purpose — drop its persisted row so it does
    // NOT come back next launch. (`kill_all` on app exit deliberately skips
    // this, which is exactly what distinguishes "forget" from "remember".)
    forget_terminal(&app, &id);
    Ok(())
}

/// Every live terminal, so the view can re-adopt the ones already running
/// after the user navigates away and back.
#[tauri::command]
pub fn list_terminals<R: Runtime>(app: AppHandle<R>) -> Result<Vec<TerminalInfo>, String> {
    let registry = app.state::<TerminalRegistry>();
    let map = registry.0.lock().map_err(|e| e.to_string())?;
    Ok(map.values().map(|h| h.info.clone()).collect())
}

struct PersistedTerminal {
    target_kind: String,
    source_id: String,
    account_id: Option<String>,
    session_uuid: Option<String>,
    display_name: Option<String>,
}

/// Reopens the terminals that were open when Hangar last closed — the first
/// thing the terminals view calls on mount. Runs exactly once per app run
/// (see `RestoreGuard`); later mounts get the live list instead.
///
/// Each row reopens on the same target under the same account, and **resumes
/// its conversation** when the transcript is still on disk (`--resume`),
/// starting fresh otherwise. A row that can't be restored — its project/session
/// gone, its account deleted, its worktree removed — is dropped without
/// aborting the rest. Reopening re-persists a fresh row (via `open_terminal`),
/// so the set survives the *next* restart too.
#[tauri::command]
pub fn restore_terminals<R: Runtime>(app: AppHandle<R>, cols: u16, rows: u16) -> Result<Vec<TerminalInfo>, String> {
    if let Some(guard) = app.try_state::<RestoreGuard>() {
        if guard.0.swap(true, Ordering::SeqCst) {
            return list_terminals(app);
        }
    }

    // Read the persisted set and clear it — `open_terminal` writes fresh rows
    // for whatever actually reopens, so a row that fails to restore simply
    // doesn't come back rather than lingering forever.
    let persisted: Vec<PersistedTerminal> = {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare("SELECT target_kind, source_id, account_id, session_uuid, display_name FROM open_terminals ORDER BY created_at ASC")
            .map_err(|e| e.to_string())?;
        let collected: Vec<PersistedTerminal> = stmt
            .query_map([], |r| {
                Ok(PersistedTerminal { target_kind: r.get(0)?, source_id: r.get(1)?, account_id: r.get(2)?, session_uuid: r.get(3)?, display_name: r.get(4)? })
            })
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .collect();
        conn.execute("DELETE FROM open_terminals", []).map_err(|e| e.to_string())?;
        collected
    };

    let mut restored = Vec::new();
    for row in persisted {
        let kind = match row.target_kind.as_str() {
            "project" => TargetKind::Project,
            "session" => TargetKind::Session,
            _ => continue,
        };
        let (init, session_uuid) = decide_restore_init(&app, &row);
        let target = TerminalTarget { kind, id: row.source_id.clone() };
        match open_terminal(app.clone(), target, row.account_id.clone(), init, session_uuid, None, cols, rows) {
            Ok(mut info) => {
                // The name the user gave this panel is part of what "restore"
                // means — without it, three panels on one project come back
                // indistinguishable.
                if let Some(name) = row.display_name.clone() {
                    let _ = rename_terminal(app.clone(), info.id.clone(), name.clone());
                    info.display_name = Some(name);
                }
                restored.push(info);
            }
            Err(e) => log::info!("restore: skipping terminal for {} ({}): {e}", row.target_kind, row.source_id),
        }
    }
    Ok(restored)
}

/// How to reopen a persisted terminal: resume its conversation when the
/// transcript is still there, otherwise a fresh session, otherwise (no account)
/// a plain shell.
fn decide_restore_init<R: Runtime>(app: &AppHandle<R>, row: &PersistedTerminal) -> (Option<InitCommand>, Option<String>) {
    let Some(account_id) = &row.account_id else {
        return (None, None); // plain shell
    };
    if let Some(uuid) = &row.session_uuid {
        let config_dir = app
            .try_state::<DbConnection>()
            .and_then(|db| db.0.lock().ok().and_then(|conn| load_account(&conn, account_id).ok()))
            .map(|a| a.config_dir_path);
        if let Some(config_dir) = config_dir {
            if crate::handoff::transcript_exists(&config_dir, uuid) {
                return (Some(InitCommand::Resume), Some(uuid.clone()));
            }
        }
    }
    // Account known but nothing to resume — start a fresh session under it.
    (Some(InitCommand::Start), None)
}

/// Every open terminal that carries an account, as `(target kind, target id,
/// account id)` — enough for the poller to know both which accounts are in use
/// and which project each of those terminals could rotate within.
fn account_terminals<R: Runtime>(app: &AppHandle<R>) -> Vec<(&'static str, String, String)> {
    let Some(registry) = app.try_state::<TerminalRegistry>() else { return Vec::new() };
    let Ok(map) = registry.0.lock() else { return Vec::new() };
    map.values()
        .filter_map(|h| {
            let account_id = h.info.account_id.clone()?;
            let kind = match h.info.kind {
                TargetKind::Project => "project",
                TargetKind::Session => "session",
            };
            Some((kind, h.info.source_id.clone(), account_id))
        })
        .collect()
}

/// Reopens a terminal under a different account — what the "reopen in <other
/// account>" button on the out-of-quota banner does. Closes the old shell and
/// starts one on the same target (same project/worktree) under the new account.
///
/// **Keeps the conversation** when it can: for a claude→claude switch of a
/// terminal whose session id Hangar owns, it copies that transcript into the new
/// account's config dir and resumes it (`handoff::transfer_claude_session` +
/// `--resume`), so the agent picks up where it left off. Falls back to a fresh
/// session when there's nothing to carry (a terminal that never conversed, a
/// pre-session-id terminal, or a non-claude account).
#[tauri::command]
pub fn reopen_terminal_with_account<R: Runtime>(app: AppHandle<R>, id: String, account_id: String, cols: u16, rows: u16) -> Result<TerminalInfo, String> {
    match plan_conversation_handoff(&app, &id, &account_id) {
        Some(uuid) => reopen_with_account_inner(app, id, account_id, Some(InitCommand::Resume), Some(uuid), cols, rows),
        // An unauthenticated target would strand the user; `Start` vs. `login`
        // is the picker's call, but here the account is already vetted.
        None => reopen_with_account_inner(app, id, account_id, Some(InitCommand::Start), None, cols, rows),
    }
}

/// Decides whether reopening `terminal_id` under `to_account_id` can keep the
/// conversation, and if so does the transcript copy and returns the uuid to
/// resume. `None` means fall back to a fresh session. The copy happens here,
/// before the new terminal opens, so `claude --resume <uuid>` finds the file.
///
/// Copy-not-move (see `handoff`): even if the later open fails, the source
/// account keeps its transcript. And because every account switch in Hangar
/// goes through this transfer, the newest copy always wins — there's no path
/// that resumes a stale copy directly, which is what the divergent-futures
/// worry in the continuity doc was about.
fn plan_conversation_handoff<R: Runtime>(app: &AppHandle<R>, terminal_id: &str, to_account_id: &str) -> Option<String> {
    let (session_uuid, from_account_id) = {
        let registry = app.try_state::<TerminalRegistry>()?;
        let map = registry.0.lock().ok()?;
        let handle = map.get(terminal_id)?;
        (handle.info.session_uuid.clone()?, handle.info.account_id.clone()?)
    };

    let db = app.try_state::<DbConnection>()?;
    let (from, to) = {
        let conn = db.0.lock().ok()?;
        (load_account(&conn, &from_account_id).ok()?, load_account(&conn, to_account_id).ok()?)
    };
    // Cross-provider isn't a file copy — that's the rendered-handoff path.
    if from.provider != "claude" || to.provider != "claude" {
        return None;
    }

    match crate::handoff::transfer_claude_session(&from.config_dir_path, &to.config_dir_path, &session_uuid) {
        Ok(()) => Some(session_uuid),
        Err(e) => {
            // Nothing to carry (never conversed, or the transcript is gone) —
            // a fresh session is the honest outcome, not an error.
            log::info!("reopen: no conversation to carry for terminal {terminal_id}: {e}");
            None
        }
    }
}

/// `reopen_terminal_with_account`'s body, with the init command and session id
/// as parameters so the tests can exercise the swap — which terminal ends up in
/// the registry, under which account, and what lands in `rotation_events` —
/// without launching a real interactive `claude`.
fn reopen_with_account_inner<R: Runtime>(app: AppHandle<R>, id: String, account_id: String, init: Option<InitCommand>, session_uuid: Option<String>, cols: u16, rows: u16) -> Result<TerminalInfo, String> {
    // Capture the old terminal's target and account before closing it — the
    // target is what the new one reopens against, and the old account is the
    // `from` side of the rotation event.
    let (target, from_account_id) = {
        let registry = app.state::<TerminalRegistry>();
        let map = registry.0.lock().map_err(|e| e.to_string())?;
        let handle = map.get(&id).ok_or_else(|| format!("no terminal with id {id}"))?;
        (TerminalTarget { kind: handle.info.kind, id: handle.info.source_id.clone() }, handle.info.account_id.clone())
    };
    let kind = match target.kind {
        TargetKind::Project => "project",
        TargetKind::Session => "session",
    };
    let source_id = target.id.clone();

    // Open before closing: if the new shell can't start, the user keeps the one
    // they had rather than losing both. `session_uuid` carries the conversation
    // when `plan_conversation_handoff` set up a resume.
    let info = open_terminal(app.clone(), target, Some(account_id.clone()), init, session_uuid, None, cols, rows)?;
    close_terminal(app.clone(), id)?;

    // Best-effort ledger write — the user already has their new terminal, so a
    // failure here is worth a log line, not an error that makes a completed
    // reopen look like it failed.
    if let Some(from_account_id) = from_account_id {
        let recorded = app
            .try_state::<DbConnection>()
            .ok_or_else(|| "no database".to_string())
            .and_then(|db| db.0.lock().map_err(|e| e.to_string()).and_then(|conn| crate::rotation::record_terminal_rotation(&conn, kind, &source_id, &from_account_id, &account_id)));
        if let Err(e) = recorded {
            log::warn!("reopen_terminal_with_account: couldn't record the rotation event: {e}");
        }
    }

    Ok(info)
}

/// One branch of a fan-out: a worktree name and the account that works in it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FanOutBranch {
    pub name: String,
    pub account_id: String,
}

/// Sends **one prompt to N agents, each in its own worktree under its own
/// account**, and returns the terminals.
///
/// This is the gesture other ADEs made popular, with the part that makes it
/// sustainable: N branches means N accounts, so a fan-out of five spends five
/// quotas instead of burning one five times as fast. Hangar can do that because
/// each account already has its own config dir; a tool with one globally-active
/// account cannot, no matter how many worktrees it cuts.
///
/// `base_branch` is the branch every worktree starts from, chosen in the picker
/// from the project's real branches — a fan-out that silently inherited
/// whatever was checked out would put half-finished work in every agent's diff.
///
/// Serial, not parallel, for the same two reasons the multi-terminal picker is:
/// accounts sharing a config dir must not race, and N CLIs spawning at once
/// stampedes the machine. A branch that fails is reported and the rest continue —
/// losing four worktrees because the fifth name collided would be absurd.
#[tauri::command]
pub async fn fan_out_prompt<R: Runtime>(
    app: AppHandle<R>,
    project_id: String,
    branches: Vec<FanOutBranch>,
    prompt: String,
    base_branch: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<Vec<TerminalInfo>, String> {
    if branches.is_empty() {
        return Err("a fan-out needs at least one branch".to_string());
    }
    if prompt.trim().is_empty() {
        return Err("a fan-out needs a prompt — that's the whole point of sending it to several agents".to_string());
    }

    let mut opened = Vec::new();
    let mut failures = Vec::new();
    for branch in branches {
        let session = crate::commands::sessions::create_worktree_for_project(app.clone(), project_id.clone(), branch.name.clone(), branch.account_id.clone(), None, base_branch.clone()).await;
        let session = match session {
            Ok(session) => session,
            Err(e) => {
                failures.push(format!("{}: {e}", branch.name));
                continue;
            }
        };
        let target = TerminalTarget { kind: TargetKind::Session, id: session.id };
        match open_terminal(
            app.clone(),
            target,
            Some(branch.account_id),
            Some(InitCommand::Start),
            None, // Hangar mints the session id
            Some(prompt.clone()),
            cols,
            rows,
        ) {
            Ok(info) => opened.push(info),
            Err(e) => failures.push(format!("{}: {e}", branch.name)),
        }
    }

    if opened.is_empty() {
        return Err(format!("no branch of the fan-out could start — {}", failures.join("; ")));
    }
    if !failures.is_empty() {
        // Partial success is the honest outcome: the user has working terminals
        // and deserves to know which branches didn't make it.
        log::warn!("fan_out_prompt: {} of the branches failed — {}", failures.len(), failures.join("; "));
    }
    Ok(opened)
}

/// Renames a panel. Persisted, because a name that vanishes on restart is worse
/// than no name: you'd have to re-derive which of three panels on the same
/// project was the one doing the migration.
#[tauri::command]
pub fn rename_terminal<R: Runtime>(app: AppHandle<R>, id: String, name: String) -> Result<(), String> {
    let trimmed = name.trim().to_string();
    // Empty means "go back to the project's own name", not "call it nothing".
    let display_name = (!trimmed.is_empty()).then_some(trimmed);
    {
        let registry = app.state::<TerminalRegistry>();
        let mut map = registry.0.lock().map_err(|e| e.to_string())?;
        let handle = map.get_mut(&id).ok_or_else(|| format!("no terminal with id {id}"))?;
        handle.info.display_name = display_name.clone();
    }
    let Some(db) = app.try_state::<DbConnection>() else { return Ok(()) };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE open_terminals SET display_name = ?2 WHERE id = ?1", rusqlite::params![id, display_name])
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// An open terminal already working in this session, if there is one.
///
/// Two agents in one worktree edit the same files with no idea the other
/// exists, so anything that "sends work to a terminal" should find the one
/// that's already there before opening a second.
pub(crate) fn terminal_for_session<R: Runtime>(app: &AppHandle<R>, session_id: &str) -> Option<TerminalInfo> {
    let registry = app.try_state::<TerminalRegistry>()?;
    let map = registry.0.lock().ok()?;
    map.values()
        .find(|handle| handle.info.kind == TargetKind::Session && handle.info.source_id == session_id)
        .map(|handle| handle.info.clone())
}

/// Types a line into a terminal that's already open, as if the user had.
pub(crate) fn send_line_to_terminal<R: Runtime>(app: &AppHandle<R>, id: &str, line: &str) -> Result<(), String> {
    write_terminal(app.clone(), id.to_string(), format!("{line}\n"))
}

/// One terminal's live state, for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalActivity {
    pub id: String,
    pub state: TerminalState,
    /// Whatever the program last set as the terminal title — for Claude Code
    /// that's its status line, which says what it's actually doing.
    pub title: Option<String>,
}

fn emit_state<R: Runtime>(app: &AppHandle<R>, id: &str, state: TerminalState, title: Option<String>) {
    let _ = app.emit("terminal-state", TerminalActivity { id: id.to_string(), state, title });
}

/// Every open terminal's state, for the first render. After that the UI follows
/// `terminal-state` events rather than polling.
#[tauri::command]
pub fn list_terminal_states<R: Runtime>(app: AppHandle<R>) -> Result<Vec<TerminalActivity>, String> {
    let registry = app.state::<TerminalRegistry>();
    let map = registry.0.lock().map_err(|e| e.to_string())?;
    Ok(map
        .iter()
        .map(|(id, handle)| {
            let (state, title) = handle
                .activity
                .lock()
                .map(|a| (a.state, a.title.clone()))
                .unwrap_or((TerminalState::Idle, None));
            TerminalActivity { id: id.clone(), state, title }
        })
        .collect())
}

/// Ages `Working` into `Waiting` once a terminal has been quiet long enough,
/// and tells the user when it happens.
///
/// A separate pass rather than a timer per terminal: the reader thread can only
/// react to bytes arriving, and the interesting transition is precisely the
/// absence of them. One tick over the registry costs nothing and scales to as
/// many panels as anyone will open.
///
/// The notification is the payoff of the whole feature — it's what makes five
/// agents in parallel something you can walk away from instead of something you
/// have to supervise.
pub fn tick_terminal_states<R: Runtime>(app: &AppHandle<R>) {
    let Some(registry) = app.try_state::<TerminalRegistry>() else { return };
    let mut newly_waiting = Vec::new();
    {
        let Ok(map) = registry.0.lock() else { return };
        for (id, handle) in map.iter() {
            let Ok(mut act) = handle.activity.lock() else { continue };
            if act.state == TerminalState::Working && act.last_output.elapsed() >= QUIET_BEFORE_WAITING {
                act.state = TerminalState::Waiting;
                newly_waiting.push((id.clone(), handle.info.title.clone(), act.title.clone(), handle.info.clone()));
            }
        }
    }

    for (id, panel_title, agent_title, info) in newly_waiting {
        // The turn is over, so the transcript is complete — the one moment
        // worth reading it. Polling for this would mean re-reading every open
        // terminal's file on a timer to catch a change that happens a few times
        // an hour.
        record_terminal_usage(app, &info);
        emit_state(app, &id, TerminalState::Waiting, agent_title.clone());
        // A queued run finishes when its agent stops talking — the only
        // completion signal available from outside the CLI.
        crate::run_queue::note_terminal_waiting(app, &id);
        crate::notifications::notify_if_enabled(
            app,
            |s| s.notify_session_waiting,
            "Agent is waiting",
            &agent_title.unwrap_or_else(|| format!("{panel_title} finished its turn")),
        );
    }
}

/// Files what a terminal's last turn cost, from what the CLI already wrote.
///
/// Only for terminals Hangar started with a session id of its own — without
/// one there is no transcript to find, which is the case for a plain shell and
/// for providers whose id we don't get to choose.
fn record_terminal_usage<R: Runtime>(app: &AppHandle<R>, info: &TerminalInfo) {
    let (Some(account_id), Some(session_uuid)) = (info.account_id.as_deref(), info.session_uuid.as_deref()) else { return };
    let Some(db) = app.try_state::<DbConnection>() else { return };

    let config_dir: Option<String> = {
        let Ok(conn) = db.0.lock() else { return };
        conn.query_row("SELECT config_dir FROM accounts WHERE id = ?1", rusqlite::params![account_id], |row| row.get(0)).ok()
    };
    let Some(config_dir) = config_dir else { return };

    let turns = crate::usage_meter::read_claude_transcript(&config_dir, session_uuid);
    if turns.is_empty() {
        return;
    }

    // A terminal is opened against either a project or a session, and the
    // column each id belongs in is different — putting a project id in
    // `session_id` would trip the foreign key and silently drop the row.
    let (project_id, session_id) = match info.kind {
        TargetKind::Project => (Some(info.source_id.clone()), None),
        TargetKind::Session => {
            let project = db
                .0
                .lock()
                .ok()
                .and_then(|conn| conn.query_row("SELECT project_id FROM sessions WHERE id = ?1", rusqlite::params![info.source_id], |row| row.get(0)).ok());
            (project, Some(info.source_id.clone()))
        }
    };

    let written = crate::usage_meter::record_turns(&db, account_id, project_id.as_deref(), session_id.as_deref(), &turns);
    if written > 0 {
        log::info!("recorded {written} new turn(s) of usage for account {account_id}");
    }
}

/// One task being picked up: its branch, who works it, and what to tell them.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskWorktree {
    /// The branch this task lives on. Used **verbatim** — no namespace — because
    /// it's the name the PR, the CI and the rest of the team already know.
    pub branch: String,
    /// True when the branch doesn't exist yet and has to be cut.
    pub create: bool,
    /// Only for a branch being created, and **per row**: unlike a fan-out,
    /// where a shared base is what makes the comparison mean anything, two
    /// tasks are unrelated work and can perfectly well start from different
    /// places.
    pub base_branch: Option<String>,
    pub account_id: String,
    /// What to hand the agent. Optional: sometimes you just want the worktree
    /// open and will type it yourself.
    pub prompt: Option<String>,
    /// The task this row came from, when it came from a board rather than
    /// being typed. Its body can't ride in `prompt` — that's a single-line
    /// shell argument — so it's written into the worktree and the prompt
    /// becomes a one-line pointer at it. See `task_sources::write_task_brief`.
    pub task: Option<crate::task_sources::Task>,
}

/// Opens one worktree and one agent per task, in parallel.
///
/// The opposite gesture to `fan_out_prompt`, and worth stating because the two
/// look alike in the UI: a fan-out is N attempts at **one** job that you compare
/// and then throw all but one away; this is N **different** jobs that you keep.
/// Which is why the base branch is per row here and shared there, and why the
/// branch name is used verbatim instead of being namespaced.
///
/// Serial for the same reasons as everything else that spawns CLIs: accounts
/// sharing a config dir must not race, and N of them starting at once stampedes
/// the machine. A task that fails is reported; the rest continue.
#[tauri::command]
pub async fn open_task_worktrees<R: Runtime>(
    app: AppHandle<R>,
    project_id: String,
    tasks: Vec<TaskWorktree>,
    cols: u16,
    rows: u16,
) -> Result<Vec<TerminalInfo>, String> {
    if tasks.is_empty() {
        return Err("pick at least one task".to_string());
    }

    let mut opened = Vec::new();
    let mut failures = Vec::new();
    for task in tasks {
        let branch = if task.create {
            crate::commands::sessions::WorktreeBranch::Exact { name: task.branch.clone(), base: task.base_branch.clone() }
        } else {
            crate::commands::sessions::WorktreeBranch::Existing(task.branch.clone())
        };

        let session = crate::commands::sessions::create_worktree_on_branch(app.clone(), project_id.clone(), branch, task.account_id.clone()).await;
        let session = match session {
            Ok(session) => session,
            Err(e) => {
                failures.push(format!("{}: {e}", task.branch));
                continue;
            }
        };

        // An explicit prompt wins: a row the user typed over is a row they
        // meant. Otherwise the brief goes to disk and the prompt points at it.
        let prompt = match (task.prompt.filter(|p| !p.trim().is_empty()), task.task.as_ref(), session.worktree_path.as_deref()) {
            (Some(typed), _, _) => Some(typed),
            (None, Some(brief), Some(worktree)) => Some(crate::task_sources::write_task_brief(std::path::Path::new(worktree), brief)),
            _ => None,
        };

        let target = TerminalTarget { kind: TargetKind::Session, id: session.id };
        match open_terminal(app.clone(), target, Some(task.account_id), Some(InitCommand::Start), None, prompt, cols, rows) {
            Ok(info) => opened.push(info),
            Err(e) => failures.push(format!("{}: {e}", task.branch)),
        }
    }

    if opened.is_empty() {
        return Err(format!("no task could be opened — {}", failures.join("; ")));
    }
    if !failures.is_empty() {
        log::warn!("open_task_worktrees: {} task(s) failed — {}", failures.len(), failures.join("; "));
    }
    Ok(opened)
}

/// Kills every shell on app exit — a PTY child outlives its parent happily,
/// so without this quitting Hangar leaves a login shell per terminal running
/// forever. Called from `lib.rs`'s `shutdown_children`.
pub fn kill_all<R: Runtime>(app: &AppHandle<R>) {
    let Some(registry) = app.try_state::<TerminalRegistry>() else { return };
    let Ok(mut map) = registry.0.lock() else { return };
    for (id, mut handle) in map.drain() {
        log::info!("killing terminal {id} on exit");
        let _ = handle.child.kill();
    }
}

/// Convenience for the frontend's "open a terminal for this session" button:
/// resolving the cwd needs the database, and the frontend has neither the
/// worktree path nor the project path in `SessionView`.
#[tauri::command]
pub fn resolve_terminal_target(db: State<DbConnection>, target: TerminalTarget) -> Result<String, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    resolve_target(&conn, &target).map(|(cwd, _)| cwd)
}

/// Which accounts one poll pass should scrape: every account with a terminal
/// open, plus — only for a terminal whose account is *currently* flagged
/// exhausted — the benched accounts that terminal's project is allowed to
/// rotate to.
///
/// That second half is what makes the banner's button reachable. Polling only
/// accounts in use means a benched account with no terminal is never re-checked,
/// so it stays benched, and `load_candidates` (healthy accounts only) finds
/// nothing to offer — the banner then says "no other allowed account to switch
/// to" about a pool that recovered hours ago. Scoping it to the flagged case
/// keeps the extra scrapes off the common path, where nothing is benched and
/// this returns exactly the accounts in use.
fn poll_set(conn: &rusqlite::Connection, terminals: &[(&str, String, String)]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for (kind, source_id, account_id) in terminals {
        ids.push(account_id.clone());

        let status: Option<String> = conn.query_row("SELECT status FROM accounts WHERE id = ?1", rusqlite::params![account_id], |r| r.get(0)).ok();
        if status.as_deref() != Some("exhausted") {
            continue;
        }
        let project_id = match crate::rotation::project_of_terminal(conn, kind, source_id) {
            Ok(id) => id,
            Err(e) => {
                log::warn!("poll_set: couldn't resolve the project behind terminal {kind}/{source_id}: {e}");
                continue;
            }
        };
        match crate::rotation::allowed_benched_accounts(conn, &project_id, "claude", account_id) {
            Ok(benched) => ids.extend(benched),
            Err(e) => log::warn!("poll_set: couldn't list benched accounts for project {project_id}: {e}"),
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Re-checks one account against `/usage` and writes the verdict to its
/// `status`. Returns whether the stored status actually changed, so callers know
/// whether the frontend needs telling.
fn refresh_account_status<R: Runtime>(app: &AppHandle<R>, account_id: &str) -> bool {
    let Some(db) = app.try_state::<DbConnection>() else { return false };

    let row: Option<(String, String, String)> = db
        .0
        .lock()
        .ok()
        .and_then(|conn| conn.query_row("SELECT provider, config_dir_path, status FROM accounts WHERE id = ?1", rusqlite::params![account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).ok());
    let Some((provider, config_dir, current_status)) = row else { return false };
    if provider != "claude" {
        return false;
    }
    // A config dir that isn't there can't be scraped — `claude` would come up
    // as nobody and answer about a quota that isn't this account's. Skipping
    // beats spawning a doomed process every five minutes.
    if !std::path::Path::new(&config_dir).is_dir() {
        return false;
    }

    let stats = match crate::usage_scraper::cached_usage_stats(app, account_id, &config_dir, false) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("usage check: /usage failed for account {account_id}: {e}");
            return false;
        }
    };
    // Keep the reading. The scrape already happened; throwing it away is what
    // made "how fast am I burning" unanswerable for the life of the project.
    if let Ok(conn) = db.0.lock() {
        if let Err(e) = crate::quota::record_sample(&conn, account_id, stats.session_pct, stats.weekly_pct, stats.context_pct) {
            log::warn!("usage check: couldn't record the sample for account {account_id}: {e}");
        }
    }

    let exhausted = crate::usage_scraper::is_exhausted(&stats);
    let desired = if exhausted { "exhausted" } else { "healthy" };
    if desired == current_status {
        return false;
    }

    let Ok(conn) = db.0.lock() else { return false };
    let now = chrono::Utc::now().to_rfc3339();
    // `exhausted_at` is set and cleared alongside the flag — it's what
    // cooldown recovery reads, so a flag written without it would never lift.
    let written = if exhausted {
        conn.execute("UPDATE accounts SET status = 'exhausted', exhausted_at = ?2, updated_at = ?2 WHERE id = ?1", rusqlite::params![account_id, now])
    } else {
        conn.execute("UPDATE accounts SET status = 'healthy', exhausted_at = NULL, updated_at = ?2 WHERE id = ?1", rusqlite::params![account_id, now])
    };
    match written {
        Ok(_) => {
            // The percentages go in the log on purpose: benching an account is
            // visible to the user as a banner, and without the numbers behind
            // it a wrong verdict from this deliberately fragile scrape is
            // undiagnosable after the fact.
            log::info!("usage check: account {account_id} is now {desired} (session {:?}, weekly {:?})", stats.session_pct, stats.weekly_pct);
            true
        }
        Err(e) => {
            log::warn!("usage check: couldn't record {desired} for account {account_id}: {e}");
            false
        }
    }
}

/// One poll pass: lift any expired benching, then re-check the accounts that
/// matter (`poll_set`) against `/usage` and flip each `status` per
/// `usage_scraper::is_exhausted`. Emits `terminal-accounts-updated` when
/// anything moved, so the frontend re-evaluates every panel's banner.
///
/// Recovery runs first and unconditionally — it needs no scrape, and it's the
/// only thing that lifts a flag from an account nobody has a terminal open for.
/// Called on a timer from `lib.rs`, and once at startup, which is when a flag
/// left over from a previous run gets cleared.
pub fn poll_account_usage<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DbConnection>() else { return };
    let mut any_change = false;

    match db.0.lock() {
        Ok(conn) => match crate::rotation::recover_cooled_down_accounts(&conn) {
            Ok(recovered) => any_change |= recovered > 0,
            Err(e) => log::warn!("poll_account_usage: cooldown recovery failed: {e}"),
        },
        Err(e) => log::warn!("poll_account_usage: database is unavailable: {e}"),
    }

    let terminals = account_terminals(app);
    let to_check = match db.0.lock() {
        Ok(conn) if !terminals.is_empty() => poll_set(&conn, &terminals),
        _ => Vec::new(),
    };
    for account_id in to_check {
        any_change |= refresh_account_status(app, &account_id);
    }

    if any_change {
        // The frontend reloads accounts (status included) and re-evaluates each
        // terminal's banner off this.
        let _ = app.emit("terminal-accounts-updated", ());
    }
}

/// Checks one account's quota on a background thread, for the moment a terminal
/// opens against it.
///
/// Without this the banner is only ever as fresh as the 5-minute poller, so a
/// terminal opened just after launch showed whatever `status` happened to be
/// stored — including a benching inherited from an earlier run — for up to five
/// minutes. Off-thread because the check spawns a hidden `claude` and takes
/// seconds; `open_terminal` must not wait for it. `cached_usage_stats` keeps a
/// burst of opens against one account down to a single scrape.
fn spawn_account_check<R: Runtime>(app: &AppHandle<R>, account_id: String) {
    let app = app.clone();
    std::thread::spawn(move || {
        if refresh_account_status(&app, &account_id) {
            let _ = app.emit("terminal-accounts-updated", ());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn scrollback_keeps_recent_output_and_stays_under_the_cap() {
        let mut buffer = VecDeque::new();
        for _ in 0..60 {
            push_scrollback(&mut buffer, &vec![b'x'; READ_CHUNK_BYTES]);
        }
        assert!(buffer.len() <= SCROLLBACK_LIMIT_BYTES, "the ring must never exceed its cap");

        // The most recent write must survive intact — that's the whole point
        // of keeping a scrollback at all.
        push_scrollback(&mut buffer, b"the last thing printed");
        let tail: Vec<u8> = buffer.iter().rev().take(22).rev().copied().collect();
        assert_eq!(tail, b"the last thing printed");
    }

    #[test]
    fn scrollback_under_the_cap_is_left_completely_alone() {
        let mut buffer = VecDeque::new();
        push_scrollback(&mut buffer, b"hello ");
        push_scrollback(&mut buffer, b"world");
        assert_eq!(buffer.iter().copied().collect::<Vec<_>>(), b"hello world");
    }

    /// A single write larger than the whole cap must still leave the buffer
    /// valid rather than underflowing the drain arithmetic.
    #[test]
    fn a_single_oversized_write_does_not_overflow_the_ring() {
        let mut buffer = VecDeque::new();
        push_scrollback(&mut buffer, &vec![b'y'; SCROLLBACK_LIMIT_BYTES * 2]);
        assert!(buffer.len() <= SCROLLBACK_LIMIT_BYTES);
    }

    fn seed() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(crate::db::SCHEMA).unwrap();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('proj1','Casablanca','/repo/casablanca','now')", []).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('acc1','claude','sub','Test','work','','','acc1','healthy','now','now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, started_at)
             VALUES ('plain','plain-session','proj1','acc1','opus','high','ask_edits','waiting','now')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, name, project_id, account_id, model, effort, mode, status, worktree_path, worktree_kind, started_at)
             VALUES ('sub','fix-billing','proj1','acc1','opus','high','ask_edits','waiting','/repo/casablanca/.hangar/worktrees/fix-billing','linked','now')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn a_project_target_resolves_to_the_project_root() {
        let conn = seed();
        let (cwd, title) = resolve_target(&conn, &TerminalTarget { kind: TargetKind::Project, id: "proj1".into() }).unwrap();
        assert_eq!(cwd, "/repo/casablanca");
        assert_eq!(title, "Casablanca");
    }

    /// The case the feature exists for: a subsession's terminal must land
    /// inside its isolated worktree, not the shared project root.
    #[test]
    fn a_subsession_target_resolves_to_its_worktree() {
        let conn = seed();
        let (cwd, title) = resolve_target(&conn, &TerminalTarget { kind: TargetKind::Session, id: "sub".into() }).unwrap();
        assert_eq!(cwd, "/repo/casablanca/.hangar/worktrees/fix-billing");
        assert_eq!(title, "fix-billing");
    }

    #[test]
    fn a_session_without_a_worktree_falls_back_to_its_project_path() {
        let conn = seed();
        let (cwd, _) = resolve_target(&conn, &TerminalTarget { kind: TargetKind::Session, id: "plain".into() }).unwrap();
        assert_eq!(cwd, "/repo/casablanca");
    }

    #[test]
    fn an_unknown_target_is_an_error_rather_than_a_silent_default() {
        let conn = seed();
        assert!(resolve_target(&conn, &TerminalTarget { kind: TargetKind::Session, id: "nope".into() }).is_err());
        assert!(resolve_target(&conn, &TerminalTarget { kind: TargetKind::Project, id: "nope".into() }).is_err());
    }

    fn parent_env() -> Vec<(String, String)> {
        vec![
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("TERM".to_string(), "xterm-256color".to_string()),
            // Hangar's own process could well have these set — a developer
            // running `ANTHROPIC_API_KEY=... npm run tauri dev` is the
            // ordinary case, not a contrived one.
            ("CLAUDE_CONFIG_DIR".to_string(), "/inherited/config".to_string()),
            ("ANTHROPIC_API_KEY".to_string(), "sk-inherited".to_string()),
        ]
    }

    fn value_of<'a>(env: &'a [(String, String)], key: &str) -> Option<&'a str> {
        env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn account(billing: &str, config_dir: &str) -> AccountForTerminal {
        AccountForTerminal {
            provider: "claude".to_string(),
            billing: billing.to_string(),
            config_dir_path: config_dir.to_string(),
            name: "Test".to_string(),
        }
    }

    /// A plain terminal is just the parent environment — no account means no
    /// stripping, because there's nothing to be masked.
    #[test]
    fn a_terminal_with_no_account_inherits_the_environment_untouched() {
        let env = merge_env(parent_env(), &[], false);
        assert_eq!(value_of(&env, "CLAUDE_CONFIG_DIR"), Some("/inherited/config"));
        assert_eq!(value_of(&env, "ANTHROPIC_API_KEY"), Some("sk-inherited"));
    }

    /// A subscription account contributes no variables at all — its config dir
    /// travels on the command line now — so the *only* thing the environment
    /// does for it is refuse to carry someone else's identity. An inherited
    /// key would authenticate as a different account than the one displayed
    /// in the command, which is the worst possible failure here: silent and
    /// billed to the wrong place.
    #[test]
    fn an_account_scoped_terminal_drops_every_inherited_identity() {
        let env = merge_env(parent_env(), &[], true);
        assert_eq!(value_of(&env, "CLAUDE_CONFIG_DIR"), None, "the command line carries the config dir; the environment must not contradict it");
        assert_eq!(value_of(&env, "ANTHROPIC_API_KEY"), None, "an inherited key must not mask a subscription login");
        assert_eq!(value_of(&env, "PATH"), Some("/usr/bin"), "everything unrelated must survive");
    }

    /// The key is the one thing that stays ambient rather than going on the
    /// command line, precisely so it never reaches the shell's history file.
    #[test]
    fn a_key_billed_account_injects_its_own_key_over_the_inherited_one() {
        let env = merge_env(parent_env(), &[("ANTHROPIC_API_KEY".to_string(), "sk-real".to_string())], true);
        let keys: Vec<_> = env.iter().filter(|(k, _)| k == "ANTHROPIC_API_KEY").collect();
        assert_eq!(keys.len(), 1, "exactly one ANTHROPIC_API_KEY must survive");
        assert_eq!(keys[0].1, "sk-real");
    }

    /// The exact string the user types by hand — bare `claude`, no quotes
    /// around a path that doesn't need them, config dir inline.
    #[test]
    fn the_command_line_is_the_one_typed_by_hand() {
        // `claude` is installed on a dev machine but not on CI, so bail rather
        // than assert on the machine.
        let Ok(start) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Start, None, None) else { return };
        assert_eq!(start, "CLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude");
        assert!(!start.ends_with('\n'), "the newline is added when it's typed, so the same string can be displayed");

        let login = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Login, None, None).unwrap();
        assert_eq!(login, "CLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude auth login --claudeai");
    }

    /// An absolute path here would be correct but wrong: it isn't what the
    /// user types, and the whole point of showing the command is that it's
    /// theirs. The login shell's own PATH is what resolves it.
    #[test]
    fn the_command_uses_the_bare_binary_name_not_an_absolute_path() {
        let Ok(command) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Start, None, None) else { return };
        let binary = command.rsplit(' ').next().unwrap();
        assert_eq!(binary, "claude", "the binary token must be the bare name, not a resolved path: {command}");
    }

    /// `cd` first, then the command — each on its own line, both visible and
    /// individually re-runnable.
    #[test]
    fn the_script_changes_directory_before_running_the_command() {
        let Ok(script) = terminal_script("/Users/killua/Proyects/WinRadar", &account("sub", "/Users/killua/.claude-David"), InitCommand::Start, None, None) else {
            return;
        };
        assert_eq!(
            script,
            "cd /Users/killua/Proyects/WinRadar\nCLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude"
        );
    }

    /// Start with a Hangar-owned session id — this is what makes the transcript
    /// addressable later (`projects/<slug>/<uuid>.jsonl`), for restore and for
    /// copying the conversation to another account.
    #[test]
    fn start_with_a_session_uuid_pins_it_on_the_command() {
        let Ok(command) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Start, Some("11111111-2222-3333-4444-555555555555"), None) else {
            return;
        };
        assert_eq!(command, "CLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude --session-id 11111111-2222-3333-4444-555555555555");
    }

    /// Resume re-attaches the CLI to a known conversation id (verified against
    /// the real CLI: `--resume <uuid>` reads and continues that transcript).
    #[test]
    fn resume_reattaches_to_a_known_session_id() {
        let Ok(command) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Resume, Some("11111111-2222-3333-4444-555555555555"), None) else {
            return;
        };
        assert_eq!(command, "CLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude --resume 11111111-2222-3333-4444-555555555555");
    }

    /// Resume with no id is a programming error, not a silent fresh start —
    /// there's nothing to resume, and a plain `claude` would quietly lose the
    /// conversation the caller meant to reopen.
    #[test]
    fn resume_without_a_session_id_is_an_error() {
        assert!(account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Resume, None, None).is_err());
    }

    /// Start without a uuid stays the plain command — this is what the account
    /// list shows as the example, where a per-terminal id would be noise.
    #[test]
    fn start_without_a_uuid_is_the_plain_command() {
        let Ok(command) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Start, None, None) else {
            return;
        };
        assert_eq!(command, "CLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude");
    }

    /// A worktree path with a space has to survive as one argument, or `cd`
    /// would take only the first word and the CLI would run in the wrong tree.
    #[test]
    fn a_working_directory_with_a_space_is_quoted_in_the_cd() {
        let Ok(script) = terminal_script("/Users/me/My Projects/app", &account("sub", "/Users/killua/.claude-David"), InitCommand::Start, None, None) else {
            return;
        };
        assert!(script.starts_with("cd '/Users/me/My Projects/app'\n"), "got: {script}");
    }

    /// An init prompt (the PR-comments case) rides on the command line as a
    /// single quoted argument to `claude`, so Claude Code opens already
    /// working on it. The quoting must survive the spaces a real prompt has.
    #[test]
    fn a_start_prompt_becomes_a_quoted_argument_to_claude() {
        let Ok(command) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Start, None, Some("PR review feedback: fix the typo")) else {
            return;
        };
        assert_eq!(command, "CLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude 'PR review feedback: fix the typo'");
    }

    /// A prompt that itself contains a single quote must stay one argument —
    /// review comments routinely have apostrophes.
    #[test]
    fn a_prompt_with_an_apostrophe_is_escaped() {
        let Ok(command) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Start, None, Some("don't break this")) else {
            return;
        };
        assert!(command.ends_with(r#"claude 'don'\''t break this'"#), "got: {command}");
    }

    /// A blank prompt is the same as no prompt — a plain interactive session,
    /// not `claude ''`.
    #[test]
    fn a_blank_prompt_is_ignored() {
        let Ok(command) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Start, None, Some("   ")) else {
            return;
        };
        assert_eq!(command, "CLAUDE_CONFIG_DIR=/Users/killua/.claude-David claude");
    }

    /// A login command never takes a prompt — there's no session to seed yet.
    #[test]
    fn a_login_ignores_any_prompt() {
        let Ok(command) = account_command_line(&account("sub", "/Users/killua/.claude-David"), InitCommand::Login, None, Some("ignored")) else {
            return;
        };
        assert!(!command.contains("ignored"), "got: {command}");
    }

    /// The API key must never appear in the command: the shell writes every
    /// line it runs to `~/.zsh_history`, so an inline assignment would put the
    /// real key on disk in plaintext and leave it in the pane's scrollback.
    #[test]
    fn the_command_line_never_contains_an_api_key() {
        let Ok(command) = account_command_line(&account("key", "/accounts/acc2"), InitCommand::Start, None, None) else { return };
        assert!(!command.contains("ANTHROPIC_API_KEY"), "the key stays ambient, never on a line the shell records: {command}");
    }

    /// A path only gets quoted when it needs it — the command is shown to the
    /// user as *the* command, and blanket quoting would make the display
    /// noisier than what they'd type themselves.
    #[test]
    fn shell_quote_only_quotes_when_necessary() {
        assert_eq!(shell_quote("/Users/killua/.claude-David"), "/Users/killua/.claude-David");
        assert_eq!(shell_quote("/Applications/My Tools/claude"), "'/Applications/My Tools/claude'");
        assert_eq!(shell_quote("/weird/it's/claude"), r"'/weird/it'\''s/claude'");
        assert_eq!(shell_quote(""), "''", "an empty value still has to occupy an argument");
    }

    /// A config dir with a space must survive as one word, or the shell would
    /// read the tail as the command to run.
    #[test]
    fn a_config_dir_with_a_space_is_quoted_in_the_command() {
        let Ok(command) = account_command_line(&account("sub", "/Users/me/My Configs/claude-work"), InitCommand::Start, None, None) else { return };
        assert!(command.starts_with("CLAUDE_CONFIG_DIR='/Users/me/My Configs/claude-work' "), "got: {command}");
    }

    /// Codex/Gemini accounts can be created, but nothing scopes their CLI to a
    /// config dir — opening a shell that looks isolated and isn't would be
    /// worse than refusing.
    #[test]
    fn a_non_claude_account_has_no_command_rather_than_a_faked_one() {
        let codex = AccountForTerminal {
            provider: "codex".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cx".to_string(),
            name: "Codex".to_string(),
        };
        let result = account_command_line(&codex, InitCommand::Start, None, None);

        // Branching on availability rather than assuming: this asserts the
        // command's *shape*, and the shape only exists on a machine that has
        // the CLI. The alternative — assuming codex is installed — is a test
        // that passes here and fails in CI for a reason unrelated to the code.
        if crate::providers::is_provider_available("codex") {
            let line = result.expect("codex is installed here, so it must produce a command");
            assert!(line.starts_with("CODEX_HOME=/tmp/cx "), "codex isolates with its own variable, not Claude's: {line}");
            assert!(!line.contains("CLAUDE_CONFIG_DIR"), "using Claude's variable would write into the shared ~/.codex: {line}");
        } else {
            let err = result.unwrap_err();
            assert!(err.contains("codex") && err.contains("isn't installed"), "an absent CLI must say so plainly: {err}");
        }
    }

    /// The bug a user hit in a real terminal: `codex --session-id <uuid>` →
    /// `error: unexpected argument '--session-id' found`. Verified against the
    /// installed CLI: Codex has no flag to name a session, so Hangar must not
    /// pass Claude's.
    #[test]
    fn a_codex_terminal_carries_no_session_flag_because_codex_has_none() {
        let codex = AccountForTerminal {
            provider: "codex".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cx".to_string(),
            name: "Codex".to_string(),
        };
        if !crate::providers::is_provider_available("codex") {
            return;
        }

        let start = account_command_line(&codex, InitCommand::Start, Some("some-uuid"), None).unwrap();
        assert!(!start.contains("--session-id"), "codex rejects that flag outright: {start}");
        assert!(start.trim_end().ends_with("codex"), "a plain start is just the binary: {start}");

        // A prompt still rides along — `codex [PROMPT]` is positional.
        let with_prompt = account_command_line(&codex, InitCommand::Start, None, Some("fix the tests")).unwrap();
        assert!(with_prompt.contains("'fix the tests'"), "got: {with_prompt}");

        // And a resume we have no id for degrades to a fresh start rather than
        // guessing at "the most recent session", which would be a guess dressed
        // up as continuity.
        let resumed = account_command_line(&codex, InitCommand::Resume, None, None).unwrap();
        assert!(!resumed.contains("--resume"), "got: {resumed}");
    }

    /// Kimi is Claude Code pointed at Moonshot's Anthropic-compatible endpoint,
    /// which is why it inherits every feature built for Claude rather than
    /// needing an adapter. The base URL has to be on the command line for that
    /// to be true — and visible there, so the picker's preview isn't a fiction.
    #[test]
    fn a_kimi_account_runs_claude_against_moonshot() {
        let kimi = AccountForTerminal {
            provider: "kimi".to_string(),
            billing: "key".to_string(),
            config_dir_path: "/tmp/kimi".to_string(),
            name: "Kimi".to_string(),
        };
        let result = account_command_line(&kimi, InitCommand::Start, None, None);
        if crate::providers::is_provider_available("kimi") {
            let line = result.expect("kimi runs through claude, which is installed here");
            assert!(line.contains("ANTHROPIC_BASE_URL=https://api.moonshot.ai/anthropic"), "got: {line}");
            assert!(line.contains(" claude"), "the binary is claude, not a kimi CLI: {line}");
        } else {
            assert!(result.is_err());
        }
    }

    /// An API-key provider has no interactive login, and offering one would
    /// open a terminal that runs a command that doesn't exist.
    #[test]
    fn a_key_only_provider_refuses_a_login_command() {
        let kimi = AccountForTerminal {
            provider: "kimi".to_string(),
            billing: "key".to_string(),
            config_dir_path: "/tmp/kimi".to_string(),
            name: "Kimi".to_string(),
        };
        let err = account_command_line(&kimi, InitCommand::Login, None, None).unwrap_err();
        assert!(err.contains("API key"), "the message has to explain why there's no login: {err}");
    }

    #[test]
    fn a_command_without_an_account_is_refused() {
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(seed())));
        app.manage(TerminalRegistry::new());

        let result = open_terminal(
            app.handle().clone(),
            TerminalTarget { kind: TargetKind::Project, id: "proj1".into() },
            None,
            Some(InitCommand::Start),
            None,
            None,
            80,
            24,
        );
        assert!(result.is_err(), "'start the CLI' as nobody in particular isn't something Hangar can do");
    }

    #[test]
    fn opening_a_terminal_for_a_missing_directory_fails_instead_of_spawning() {
        let app = tauri::test::mock_app();
        let conn = seed();
        app.manage(crate::db::DbConnection(Mutex::new(conn)));
        app.manage(TerminalRegistry::new());

        // `seed` points proj1 at /repo/casablanca, which doesn't exist — the
        // real-world version of this is a worktree deleted off disk while its
        // session row still names it.
        let result = open_terminal(app.handle().clone(), TerminalTarget { kind: TargetKind::Project, id: "proj1".into() }, None, None, None, None, 80, 24);
        assert!(result.is_err(), "a vanished working directory must be reported, not silently spawned somewhere else");
        assert!(result.unwrap_err().contains("no longer exists"));
    }

    /// End-to-end over the real PTY: spawns an actual shell through
    /// `open_terminal`, types a command with `write_terminal`, and waits for
    /// the output to show up in the scrollback the way `attach_terminal`
    /// would replay it. Covers shell spawn, cwd, the writer, and the reader
    /// thread together — the parts that a pure unit test can't reach and
    /// where this feature would actually break.
    #[test]
    fn a_real_shell_runs_a_command_and_its_output_reaches_the_scrollback() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("hangar-test-terminal-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let conn = seed();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('tmp','Temp',?1,'now')",
            rusqlite::params![dir.to_string_lossy().to_string()],
        )
        .unwrap();

        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(conn)));
        app.manage(TerminalRegistry::new());
        let handle = app.handle().clone();

        let info = open_terminal(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp".into() }, None, None, None, None, 80, 24)
            .expect("opening a terminal in a real directory must succeed");
        assert_eq!(info.cwd, dir.to_string_lossy());

        write_terminal(handle.clone(), info.id.clone(), "echo hangar-terminal-ok\n".to_string()).unwrap();

        // A login shell sources the user's profile before it gets to our
        // command, so poll rather than assuming a fixed delay.
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = false;
        while Instant::now() < deadline {
            let registry = handle.state::<TerminalRegistry>();
            let map = registry.0.lock().unwrap();
            let buffer = map.get(&info.id).unwrap().scrollback.lock().unwrap();
            let text = String::from_utf8_lossy(&buffer.iter().copied().collect::<Vec<_>>()).to_string();
            drop(buffer);
            drop(map);
            // The echoed command line contains the marker too, so require the
            // shell's own output: the marker on a line of its own.
            if text.matches("hangar-terminal-ok").count() >= 2 {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(seen, "the shell's output must reach the scrollback so `attach_terminal` can replay it");

        // Resizing a live PTY must not error — this is what every pane does
        // on its first fit().
        resize_terminal(handle.clone(), info.id.clone(), 120, 40).unwrap();

        close_terminal(handle.clone(), info.id.clone()).unwrap();
        assert!(list_terminals(handle).unwrap().is_empty(), "a closed terminal must leave the registry");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Opening against an account reports whose credentials the shell carries,
    /// and registers the terminal.
    ///
    /// This deliberately does *not* assert that the CLI ran. It used to: an
    /// earlier version planted a fake `claude` on a temporary PATH and waited
    /// for its output. That stopped being possible when the command switched
    /// to the bare binary name — resolution now happens inside the login
    /// shell, against the user's own profile, so a test can no longer decide
    /// which `claude` wins. Faking it would mean asserting on a PATH the real
    /// code no longer consults, and letting it through would launch a real
    /// interactive Claude Code session in the test suite. The two halves are
    /// covered separately instead: `the_script_changes_directory_before_...`
    /// pins the exact text, and
    /// `a_real_shell_runs_a_command_and_its_output_reaches_the_scrollback`
    /// proves text written to the PTY reaches and runs in the shell.
    #[test]
    fn opening_against_an_account_records_whose_credentials_it_carries() {
        let dir = std::env::temp_dir().join(format!("hangar-test-account-terminal-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let conn = seed();
        conn.execute(
            "INSERT INTO projects (id, name, path, created_at) VALUES ('tmp2','Temp',?1,'now')",
            rusqlite::params![dir.to_string_lossy().to_string()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('sub1','claude','sub','Personal','personal','','/accounts/sub1','sub1','healthy','now','now')",
            [],
        )
        .unwrap();

        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(conn)));
        app.manage(TerminalRegistry::new());
        let handle = app.handle().clone();

        let info = open_terminal(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp2".into() }, Some("sub1".to_string()), None, None, None, 80, 24)
            .expect("opening against an account must succeed");
        assert_eq!(info.account_name.as_deref(), Some("Personal"), "the pane header has to show whose credentials this shell carries");
        assert_eq!(list_terminals(handle.clone()).unwrap().len(), 1);

        close_terminal(handle, info.id).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- agent state ---------------------------------------------------------

    #[test]
    fn a_title_is_read_from_either_osc_form() {
        // BEL-terminated, the common one.
        assert_eq!(parse_osc_title(b"\x1b]0;fixing the rounding\x07").as_deref(), Some("fixing the rounding"));
        // ST-terminated (ESC \), which some programs prefer.
        assert_eq!(parse_osc_title(b"\x1b]2;running tests\x1b\\").as_deref(), Some("running tests"));
    }

    /// Titles arrive mixed into ordinary output, often several in one read —
    /// the last one is the current state of the world.
    #[test]
    fn the_last_title_in_a_chunk_wins_and_surrounding_output_is_ignored() {
        let chunk = b"some output\x1b]0;first\x07more output\x1b]0;second\x07trailing";
        assert_eq!(parse_osc_title(chunk).as_deref(), Some("second"));
    }

    /// A sequence split across two reads must not produce half a title. The
    /// next chunk carries a complete one anyway, so dropping the fragment is
    /// right; inventing a truncated title would put garbage in the UI.
    #[test]
    fn an_unterminated_sequence_yields_nothing() {
        assert_eq!(parse_osc_title(b"\x1b]0;half a tit"), None);
        assert_eq!(parse_osc_title(b"plain output with no escapes at all"), None);
        assert_eq!(parse_osc_title(b"\x1b]0;\x07"), None, "an empty title is not a title");
    }

    /// Other OSC commands share the prefix — 4 is a palette change, 8 is a
    /// hyperlink. Reading those as titles would fill the panel header with
    /// colour codes.
    #[test]
    fn other_osc_commands_are_not_mistaken_for_titles() {
        assert_eq!(parse_osc_title(b"\x1b]4;1;rgb:00/00/00\x07"), None);
        assert_eq!(parse_osc_title(b"\x1b]8;;https://example.com\x07"), None);
    }

    /// A fan-out with nothing to fan is a mistake, not an empty success: it
    /// would cut zero worktrees and report victory.
    #[test]
    fn a_fan_out_needs_at_least_one_branch() {
        let app = tauri::test::mock_app();
        let err = tauri::async_runtime::block_on(fan_out_prompt(app.handle().clone(), "proj1".to_string(), vec![], "do the thing".to_string(), None, 80, 24)).unwrap_err();
        assert!(err.contains("at least one branch"), "got: {err}");
    }

    /// And a fan-out with no prompt is the same mistake from the other side —
    /// N worktrees with nothing to work on. Checked before anything is created,
    /// so a blank prompt can't leave real branches behind.
    #[test]
    fn a_fan_out_needs_a_prompt() {
        let app = tauri::test::mock_app();
        let branches = vec![FanOutBranch { name: "one".to_string(), account_id: "sub1".to_string() }];
        let err = tauri::async_runtime::block_on(fan_out_prompt(app.handle().clone(), "proj1".to_string(), branches, "   ".to_string(), None, 80, 24)).unwrap_err();
        assert!(err.contains("needs a prompt"), "got: {err}");
    }

    /// A directory, a project pointing at it, and two accounts — the setup the
    /// out-of-quota banner acts on: one account benched, one to move to.
    fn seed_two_accounts(dir: &std::path::Path) -> Connection {
        let conn = seed();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('tmp2','Temp',?1,'now')", rusqlite::params![dir.to_string_lossy().to_string()]).unwrap();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('sub1','claude','sub','Personal','personal','','/accounts/sub1','sub1','exhausted','now','now'),
                    ('sub2','claude','sub','Work','work','','/accounts/sub2','sub2','healthy','now','now')",
            [],
        )
        .unwrap();
        conn
    }

    /// The whole point of the banner's button: the pane the user was looking at
    /// is replaced by one running under the account they picked, on the same
    /// directory, and the move is written to the rotation ledger.
    ///
    /// Passes `None` as the init command (via the inner fn) for the same reason
    /// the test above doesn't assert the CLI ran: `Start` would launch a real
    /// interactive Claude Code inside the test suite. What's being pinned here
    /// is the swap — registry, account, target, ledger — and
    /// `the_script_changes_directory_before_running_the_command` already pins
    /// the command text the real command sends.
    #[test]
    fn reopening_under_another_account_replaces_the_shell_and_records_the_rotation() {
        let dir = std::env::temp_dir().join(format!("hangar-test-reopen-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(seed_two_accounts(&dir))));
        app.manage(TerminalRegistry::new());
        let handle = app.handle().clone();

        let old = open_terminal(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp2".into() }, Some("sub1".to_string()), None, None, None, 80, 24).unwrap();
        assert_eq!(
            account_terminals(&handle),
            vec![("project", "tmp2".to_string(), "sub1".to_string())],
            "the poller learns which accounts to check, and within which project, from here"
        );

        let new = reopen_with_account_inner(handle.clone(), old.id.clone(), "sub2".to_string(), None, None, 80, 24).expect("reopening under an allowed account must succeed");

        assert_ne!(new.id, old.id, "a reopen is a new shell, not the old one relabelled");
        assert_eq!(new.account_id.as_deref(), Some("sub2"));
        assert_eq!(new.account_name.as_deref(), Some("Work"), "the header must name the account actually carrying the quota now");
        assert_eq!(new.source_id, old.source_id, "the new terminal has to land on the same target");
        assert_eq!(new.cwd, old.cwd);

        let live = list_terminals(handle.clone()).unwrap();
        assert_eq!(live.len(), 1, "the exhausted account's shell must be gone, not left running alongside");
        assert_eq!(live[0].id, new.id);
        assert_eq!(
            account_terminals(&handle),
            vec![("project", "tmp2".to_string(), "sub2".to_string())],
            "the poller must now see the new account in use, and no trace of the old one"
        );

        let event: (String, String, String) = {
            let db = handle.state::<crate::db::DbConnection>();
            let conn = db.0.lock().unwrap();
            conn.query_row("SELECT project_id, from_account_id, to_account_id FROM rotation_events", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .expect("the reopen must leave a rotation event — round-robin reads this as the account's last use")
        };
        assert_eq!(event, ("tmp2".to_string(), "sub1".to_string(), "sub2".to_string()));

        close_terminal(handle, new.id).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A project with two accounts checked in Settings → Rotation, both benched
    /// unless told otherwise — the state where the banner needs a candidate.
    fn seed_rotation_pool() -> Connection {
        let conn = seed();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at, exhausted_at)
             VALUES ('sub1','claude','sub','Personal','personal','','/accounts/sub1','sub1','exhausted','now','now','now'),
                    ('sub2','claude','sub','Work','work','','/accounts/sub2','sub2','exhausted','now','now','now'),
                    ('sub3','claude','sub','Spare','work','','/accounts/sub3','sub3','exhausted','now','now','now')",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO project_allowed_accounts (project_id, account_id, allowed) VALUES ('proj1','sub1',1), ('proj1','sub2',1), ('proj1','sub3',0)", []).unwrap();
        conn
    }

    /// The common case: nothing is benched, so a pass scrapes exactly the
    /// accounts with a terminal open and nothing else.
    #[test]
    fn a_poll_pass_checks_only_the_accounts_in_use() {
        let conn = seed_rotation_pool();
        conn.execute("UPDATE accounts SET status = 'healthy', exhausted_at = NULL", []).unwrap();

        let terminals = vec![("project", "proj1".to_string(), "sub1".to_string())];
        assert_eq!(poll_set(&conn, &terminals), vec!["sub1".to_string()]);
    }

    /// When an in-use account *is* benched, the pass widens to the accounts its
    /// project could rotate to — otherwise they stay benched unchecked forever
    /// and the banner has nothing to offer. `sub3` is in the pool but unchecked
    /// in Settings, so it stays out.
    #[test]
    fn a_benched_account_in_use_pulls_its_projects_candidates_into_the_pass() {
        let conn = seed_rotation_pool();
        let terminals = vec![("project", "proj1".to_string(), "sub1".to_string())];
        assert_eq!(poll_set(&conn, &terminals), vec!["sub1".to_string(), "sub2".to_string()]);
    }

    /// A session-kind terminal (a PR worktree) resolves through its session to
    /// the same project, so it widens the same way.
    #[test]
    fn a_session_terminal_resolves_its_project_for_the_candidate_sweep() {
        let conn = seed_rotation_pool();
        let terminals = vec![("session", "sub".to_string(), "sub1".to_string())];
        assert_eq!(poll_set(&conn, &terminals), vec!["sub1".to_string(), "sub2".to_string()]);
    }

    /// Two terminals on the same account must not queue two scrapes of it.
    #[test]
    fn the_same_account_is_never_checked_twice_in_one_pass() {
        let conn = seed_rotation_pool();
        conn.execute("UPDATE accounts SET status = 'healthy', exhausted_at = NULL", []).unwrap();

        let terminals = vec![("project", "proj1".to_string(), "sub1".to_string()), ("session", "sub".to_string(), "sub1".to_string())];
        assert_eq!(poll_set(&conn, &terminals), vec!["sub1".to_string()]);
    }

    /// A reopen that can't start must leave the user with the terminal they
    /// had. The command is resolved before anything is spawned, so a target
    /// account Hangar can't run as (here a non-`claude` provider) fails there —
    /// and must fail without having closed the original or logged a rotation
    /// that never happened.
    #[test]
    fn a_reopen_that_cannot_start_leaves_the_original_terminal_alive() {
        let dir = std::env::temp_dir().join(format!("hangar-test-reopen-fail-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let conn = seed_two_accounts(&dir);
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('cx1','codex','sub','Codex','work','','/accounts/cx1','cx1','healthy','now','now')",
            [],
        )
        .unwrap();

        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(conn)));
        app.manage(TerminalRegistry::new());
        let handle = app.handle().clone();

        let old = open_terminal(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp2".into() }, Some("sub1".to_string()), None, None, None, 80, 24).unwrap();

        // An account that doesn't exist, rather than one whose CLI is missing:
        // the invariant under test is the rollback, and tying it to which CLIs
        // happen to be installed made it pass here and fail elsewhere — which
        // is exactly what happened once codex started working.
        let err = reopen_with_account_inner(handle.clone(), old.id.clone(), "no-such-account".to_string(), Some(InitCommand::Start), None, 80, 24)
            .unwrap_err();
        assert!(!err.is_empty(), "a reopen that can't start has to say something");

        let live = list_terminals(handle.clone()).unwrap();
        assert_eq!(live.len(), 1, "a failed reopen must not leave the user with no terminal at all");
        assert_eq!(live[0].id, old.id, "the original shell is the one that has to survive");

        let events: i64 = {
            let db = handle.state::<crate::db::DbConnection>();
            let conn = db.0.lock().unwrap();
            conn.query_row("SELECT COUNT(*) FROM rotation_events", [], |r| r.get(0)).unwrap()
        };
        assert_eq!(events, 0, "nothing rotated, so nothing may be recorded");

        close_terminal(handle, old.id).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    // -- persistence (Layer 1) ----------------------------------------------

    fn info_for(id: &str, kind: TargetKind, source_id: &str, account_id: Option<&str>, session_uuid: Option<&str>) -> TerminalInfo {
        TerminalInfo {
            id: id.to_string(),
            title: "t".to_string(),
            cwd: "/repo".to_string(),
            kind,
            source_id: source_id.to_string(),
            account_name: None,
            account_id: account_id.map(str::to_string),
            session_uuid: session_uuid.map(str::to_string),
            display_name: None,
        }
    }

    fn persisted_ids(app: &tauri::App<tauri::test::MockRuntime>) -> Vec<String> {
        let db = app.state::<crate::db::DbConnection>();
        let conn = db.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM open_terminals ORDER BY created_at").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
    }

    #[test]
    fn persist_then_forget_round_trips() {
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(seed())));
        let handle = app.handle();

        persist_terminal(handle, &info_for("t1", TargetKind::Project, "proj1", Some("acc1"), Some("uuid-1")));
        assert_eq!(persisted_ids(&app), vec!["t1".to_string()], "an opened terminal must be remembered");

        forget_terminal(handle, "t1");
        assert!(persisted_ids(&app).is_empty(), "a user-closed terminal must be forgotten");
    }

    /// The distinction the whole feature hinges on: closing a terminal forgets
    /// it, but the app shutting down (`kill_all`) must NOT — otherwise a restart
    /// would come back empty, which is the bug this fixes.
    #[test]
    fn kill_all_keeps_the_persisted_rows_but_close_drops_them() {
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(seed())));
        app.manage(TerminalRegistry::new());
        let handle = app.handle();

        persist_terminal(handle, &info_for("t1", TargetKind::Project, "proj1", Some("acc1"), None));
        persist_terminal(handle, &info_for("t2", TargetKind::Session, "sub", Some("acc1"), None));

        kill_all(handle);
        assert_eq!(persisted_ids(&app).len(), 2, "app shutdown must remember every open terminal");

        // A user closing one drops just that row (the registry is empty here,
        // so this exercises the persistence side).
        close_terminal(handle.clone(), "t1".to_string()).unwrap();
        assert_eq!(persisted_ids(&app), vec!["t2".to_string()], "closing one must forget only it");
    }

    // -- restore init decision (Layer 3) ------------------------------------

    #[test]
    fn restore_starts_a_plain_shell_when_there_was_no_account() {
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(seed())));
        let row = PersistedTerminal { target_kind: "project".into(), source_id: "proj1".into(), account_id: None, session_uuid: None, display_name: None };
        assert_eq!(decide_restore_init(app.handle(), &row), (None, None));
    }

    #[test]
    fn restore_resumes_when_the_transcript_is_still_on_disk_else_starts_fresh() {
        // A real config dir with a planted transcript for the session.
        let cfg = std::env::temp_dir().join(format!("hangar-restore-cfg-{}", Uuid::new_v4()));
        let uuid = "abcd1234-0000-0000-0000-000000000000";
        std::fs::create_dir_all(cfg.join("projects").join("-repo")).unwrap();
        std::fs::write(cfg.join("projects").join("-repo").join(format!("{uuid}.jsonl")), "convo\n").unwrap();

        let conn = seed();
        conn.execute(
            "INSERT INTO accounts (id, provider, billing, name, scope, plan_label, config_dir_path, credential_ref, status, created_at, updated_at)
             VALUES ('accR','claude','sub','R','work','',?1,'accR','healthy','now','now')",
            rusqlite::params![cfg.to_string_lossy().to_string()],
        )
        .unwrap();
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(conn)));
        let handle = app.handle();

        // Transcript present → resume that exact session.
        let with = PersistedTerminal { target_kind: "project".into(), source_id: "proj1".into(), account_id: Some("accR".into()), session_uuid: Some(uuid.into()), display_name: None };
        assert_eq!(decide_restore_init(handle, &with), (Some(InitCommand::Resume), Some(uuid.to_string())));

        // Same account, a session id with no transcript on disk → fresh start.
        let without = PersistedTerminal { target_kind: "project".into(), source_id: "proj1".into(), account_id: Some("accR".into()), session_uuid: Some("no-such-uuid".into()), display_name: None };
        assert_eq!(decide_restore_init(handle, &without), (Some(InitCommand::Start), None));

        let _ = std::fs::remove_dir_all(&cfg);
    }
}
