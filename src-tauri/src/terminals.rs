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

/// How often a terminal's pending output is handed to the pane displaying it.
/// One frame at 60Hz: below what anyone can see, and it turns an unbounded
/// number of IPC round trips per second into at most 60 — see `TerminalSink`.
pub const OUTPUT_FLUSH_INTERVAL: Duration = Duration::from_millis(16);

/// Ceiling on output waiting to be handed to a pane. Reached only when the
/// webview can't keep up with the PTY, at which point the oldest bytes are
/// dropped rather than growing the buffer without limit — the same trade
/// `push_scrollback` makes with `SCROLLBACK_LIMIT_BYTES`, and the same one a
/// terminal emulator makes when a program outruns its screen.
const PENDING_OUTPUT_LIMIT_BYTES: usize = 512 * 1024;

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
    /// Folded in the grid. Sent from here so a restart can put the panels back
    /// the way they were left: the frontend cannot remember this on its own,
    /// because restore reopens every terminal with a new id and anything keyed
    /// by the old one is already stale.
    pub minimized: bool,
    /// Continues on its own once its usage limit resets — the user's per-panel
    /// choice (`auto_continue.rs`). Decides the `--settings` its CLI started
    /// with, so changing it relaunches the CLI.
    pub auto_continue: bool,
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
            if let Some(end) = osc_payload_end(chunk, start) {
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

/// Where an OSC payload started at `start` ends: BEL (0x07) or ST (`ESC \`).
/// `None` means the sequence didn't terminate within `chunk` — shared by
/// `parse_osc_title` and `bridge::redact`'s ANSI stripper, which both need to
/// recognize the same terminator without duplicating the scan.
pub(crate) fn osc_payload_end(chunk: &[u8], start: usize) -> Option<usize> {
    let mut end = start;
    while end < chunk.len() && chunk[end] != 0x07 && !(chunk[end] == 0x1b && end + 1 < chunk.len() && chunk[end + 1] == b'\\') {
        end += 1;
    }
    (end < chunk.len()).then_some(end)
}

/// One chunk of terminal output on its way to the frontend.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalChunk {
    /// Base64 of the raw PTY bytes — see the module docs for why this isn't a
    /// plain `String`.
    pub base64: String,
}

/// Where a terminal's output is going, plus what hasn't gone yet.
///
/// The bytes wait here instead of being sent per read because every `send` is
/// an IPC round trip: a `TerminalChunk` carrying 8 KiB of base64 is past
/// Tauri's `MAX_JSON_DIRECT_EXECUTE_THRESHOLD`, so each one costs a
/// `webview.eval` *and* a `fetch` back — on the thread that draws the window.
/// A talkative agent was buying ~128 of those a second, per terminal, and the
/// window stopped repainting. `tick_terminal_output` drains this on a timer
/// instead, which caps the traffic at one message per tick no matter how fast
/// the PTY talks.
///
/// Channel and buffer share one mutex so `attach_terminal` can install a sink
/// with nothing pending in the same critical section where it snapshots the
/// scrollback — two locks would leave a gap in which the reader could append
/// bytes that the snapshot already carried, and the pane would print them
/// twice.
struct TerminalSink {
    channel: Channel<TerminalChunk>,
    pending: Vec<u8>,
}

struct TerminalHandle {
    /// Kept alive purely so `resize` has something to call: dropping the
    /// master closes the PTY out from under the child.
    master: Box<dyn MasterPty + Send>,
    /// Shared so the deferred init-command thread can type into the shell
    /// after it's ready (see `open_terminal`), not only the keystroke command.
    /// Encolar aquí es lo único que hace `write_terminal`.
    ///
    /// El writer del PTY vive dentro de un hilo propio y nadie más lo toca. Antes
    /// era un `Arc<Mutex<…>>` que se bloqueaba **con el candado global del
    /// registro puesto**, y como escribir a un PTY se para cuando el hijo no
    /// vacía su entrada, una escritura atascada congelaba a la vez el hilo de
    /// IPC y los tres ticks que ese candado protege — o sea, la app entera.
    ///
    /// Acotado: un PTY parado con un canal infinito es memoria sin techo.
    writes: std::sync::mpsc::SyncSender<Vec<u8>>,
    child: Box<dyn Child + Send + Sync>,
    scrollback: Arc<Mutex<VecDeque<u8>>>,
    /// The channel of whoever is currently displaying this terminal, or
    /// `None` while nobody is. The reader thread keeps filling `scrollback`
    /// either way, so a background terminal never loses output.
    sink: Arc<Mutex<Option<TerminalSink>>>,
    /// Live state, written by the reader thread and by the watcher that ages
    /// `Working` into `Waiting`. Shared rather than stored on `info` because
    /// both of those run outside any command.
    activity: Arc<Mutex<Activity>>,
    /// Redacted, bounded tail for terminals without a session — see
    /// `bridge::redact`. Kept for every terminal, not only `kind='project'`
    /// ones: a session-backed terminal's PTY still exists underneath
    /// `send_message`, it just has no mobile screen wired to read this today.
    mobile_tail: Arc<Mutex<crate::bridge::redact::PtyTail>>,
    auto: Arc<Mutex<AutoContinue>>,
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

/// A terminal's auto-continue state (see `auto_continue.rs`), shared by the
/// reader thread that notices Claude Code giving up, the keystroke path that
/// cancels on the user's own input, and the tick that types the prompt.
#[derive(Debug, Default)]
struct AutoContinue {
    enabled: bool,
    /// When Hangar will type the continuation. Only ever set in the two cases
    /// Claude Code declines to wait itself.
    resume_at: Option<chrono::DateTime<chrono::Utc>>,
    /// `resume_at` is a retry, not the reported reset: there was no
    /// `rate_limits` reading to take the time from.
    estimated: bool,
    /// The check changed mid-turn; the CLI is relaunched with the new
    /// `--settings` once the turn ends rather than cut off.
    pending_relaunch: bool,
    /// Recent plain text, so a phrase split across two reads is still seen.
    recent: String,
}

/// With no reset time to go on, try again after this long. The prompt either
/// runs or meets the limit again, and then Claude Code says so and this re-arms.
const AUTO_CONTINUE_RETRY: chrono::Duration = chrono::Duration::minutes(30);

/// Never sooner than this after arming, however stale the reading: a reset
/// time already in the past would otherwise fire on the next tick, into a
/// limit that may not have lifted.
const AUTO_CONTINUE_MIN_WAIT: chrono::Duration = chrono::Duration::minutes(2);

/// What the pane shows about a terminal's auto-continue.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutoContinueStatus {
    pub id: String,
    pub enabled: bool,
    pub resume_at: Option<String>,
    pub estimated: bool,
    pub pending_relaunch: bool,
}

fn auto_status(id: &str, auto: &AutoContinue) -> AutoContinueStatus {
    AutoContinueStatus {
        id: id.to_string(),
        enabled: auto.enabled,
        resume_at: auto.resume_at.map(|t| t.to_rfc3339()),
        estimated: auto.estimated,
        pending_relaunch: auto.pending_relaunch,
    }
}

fn emit_auto<R: Runtime>(app: &AppHandle<R>, id: &str, auto: &AutoContinue) {
    let _ = app.emit("terminal-auto-continue", auto_status(id, auto));
}

/// When to type the continuation for an account whose CLI just gave up: the
/// reported reset plus jitter, never sooner than `AUTO_CONTINUE_MIN_WAIT`, and a
/// plain retry when nothing was reported. Returns the time and whether it's the
/// estimated kind.
fn arm_time(root: Option<&std::path::Path>, account_id: Option<&str>) -> (chrono::DateTime<chrono::Utc>, bool) {
    let now = chrono::Utc::now();
    let reported = root
        .zip(account_id)
        .and_then(|(root, account)| crate::auto_continue::latest_rate_limits(root, account))
        .and_then(|limits| crate::auto_continue::resume_at(&limits, crate::auto_continue::random_jitter()));
    match reported {
        Some(at) => (at.max(now + AUTO_CONTINUE_MIN_WAIT), false),
        None => (now + AUTO_CONTINUE_RETRY, true),
    }
}

/// Cuántas escrituras pendientes se aguantan por terminal antes de decir que no.
///
/// Generoso para tecleo humano y para un pegado grande, y aun así un techo: si
/// el hijo dejó de leer del todo, esto es lo que separa «se acumula un rato» de
/// «la memoria crece sin fin». Al llenarse, `write_terminal` devuelve error en
/// vez de bloquear o tragarse la tecla en silencio.
const WRITE_QUEUE_DEPTH: usize = 4096;

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

/// The narrowest terminal an agent CLI is given, however narrow its pane is.
///
/// Not tidiness — a measured crash. opencode's TUI segfaults inside Bun when the
/// terminal is 28 columns or fewer, and renders fine from 30 up; the row count
/// makes no difference (5×20, 10×20 and 20×20 all die, 24×40 does not). A pane
/// in a six-up grid, or one caught before the frontend's first `fit()` lands, is
/// exactly that narrow, which is why this looked like "opencode doesn't open".
///
/// Applied to every provider rather than special-cased: no agent CLI is usable
/// below this width, so the only cost is that a hopelessly narrow pane wraps
/// text wider than it can show — while the benefit is that no CLI can be handed
/// a size that kills it. 40 rather than the measured 30 leaves room for the
/// threshold to move in a future version without breaking this again.
const MIN_PTY_COLS: u16 = 40;

/// The shell to launch. `$SHELL` is what the user actually chose; the
/// fallback only matters for a process started without it.
#[cfg(unix)]
pub(crate) fn user_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string())
}

/// The shell to launch on Windows — Git's `bash.exe` whenever it can be found.
///
/// This is the load-bearing decision behind Windows support, and it is not the
/// obvious one. `terminal_script_full` below emits POSIX: `VAR=value binary`
/// prefixes, `cd` lines, `&&`. PowerShell understands none of that, so pointing
/// this at `powershell.exe` would mean writing a second script generator and a
/// second copy of every test that covers the first.
///
/// Git for Windows ships a full bash, and Hangar already cannot function
/// without git — worktrees are how a subsession exists at all. So the shell
/// that keeps every generated script valid is one the machine necessarily
/// already has.
///
/// `powershell.exe` is the last resort rather than an equal option: a terminal
/// that opens and shows a prompt is more useful than one that fails to spawn,
/// even if the account-scoped command written into it won't parse.
#[cfg(windows)]
pub(crate) fn user_shell() -> String {
    // `$SHELL` first for the same reason as on Unix: if the user has one, it's
    // an answer they chose. Git Bash and MSYS both set it.
    if let Ok(shell) = std::env::var("SHELL") {
        if !shell.is_empty() && std::path::Path::new(&shell).is_file() {
            return shell;
        }
    }
    if let Some(bash) = git_bash_path() {
        return bash;
    }
    "powershell.exe".to_string()
}

/// Git's `bash.exe`, looked up the same way `providers::resolve_binary` looks up
/// a CLI: PATH first, then the places the installer actually puts it.
///
/// Deliberately *not* `sh.exe` from `Git\usr\bin`: that one is on the MSYS
/// side and behaves differently about paths and `-l`.
#[cfg(windows)]
pub(crate) fn git_bash_path() -> Option<String> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("bash.exe");
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().into_owned());
            }
        }
    }
    // The two install roots Git for Windows uses — 64-bit and the 32-bit
    // installer's directory — read from the environment rather than hardcoded
    // to `C:\`, since neither is guaranteed to be on the system drive.
    for root_var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        let Ok(root) = std::env::var(root_var) else { continue };
        let candidate = std::path::Path::new(&root).join("Git").join("bin").join("bash.exe");
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    // Per-user install ("Git for Windows" installed without admin rights).
    if let Some(home) = crate::app_data::home_dir() {
        let candidate = std::path::Path::new(&home).join("AppData/Local/Programs/Git/bin/bash.exe");
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
}

/// Whether `user_shell()` speaks POSIX — which decides both the login flag the
/// PTY passes it and how `run_setup_command` hands it a one-liner.
///
/// On Unix this is always true. On Windows it's true exactly when
/// `user_shell()` found a bash rather than falling through to PowerShell.
pub(crate) fn shell_is_posix() -> bool {
    #[cfg(unix)]
    {
        true
    }
    #[cfg(windows)]
    {
        !user_shell().to_ascii_lowercase().ends_with("powershell.exe")
    }
}

/// How to hand this shell a single command to run and exit — `sh -c` and its
/// equivalent. Used by `run_setup_command`, which used to hardcode `sh`.
pub(crate) fn shell_command_args(command: &str) -> Vec<String> {
    if shell_is_posix() {
        vec!["-c".to_string(), command.to_string()]
    } else {
        // `-NoProfile` because a setup command's output is parsed, and a
        // profile that prints a banner would end up in it.
        vec!["-NoProfile".to_string(), "-Command".to_string(), command.to_string()]
    }
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
///
/// `ANTHROPIC_BASE_URL` and `ANTHROPIC_AUTH_TOKEN` belong here for the same
/// reason, and matter more: inherited, they would send a subscription account's
/// terminal — and the project's code — to somebody else's endpoint.
const IDENTITY_ENV_VARS: &[&str] = &["CLAUDE_CONFIG_DIR", "ANTHROPIC_API_KEY", "ANTHROPIC_BASE_URL", "ANTHROPIC_AUTH_TOKEN"];

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
    /// Where a `custom` account points Claude Code; `None` for every provider
    /// whose endpoint is fixed by its spec.
    pub endpoint: Option<AccountEndpoint>,
}

/// The per-account half of what `ProviderSpec::extra_env` is for Kimi: the
/// endpoint lives on the account row because every custom account has its own.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct AccountEndpoint {
    pub base_url: String,
    pub model: Option<String>,
}

impl AccountEndpoint {
    /// From the two nullable `accounts` columns. No address means no endpoint,
    /// whatever the model column says.
    pub(crate) fn from_columns(base_url: Option<String>, model: Option<String>) -> Option<Self> {
        let base_url = base_url.filter(|u| !u.trim().is_empty())?;
        Some(Self { base_url, model: model.filter(|m| !m.trim().is_empty()) })
    }

    /// The variables that point Claude Code at this endpoint.
    fn env(&self) -> Vec<(&'static str, &str)> {
        let mut env = vec![("ANTHROPIC_BASE_URL", self.base_url.as_str())];
        if let Some(model) = &self.model {
            env.push(("ANTHROPIC_MODEL", model.as_str()));
        }
        env
    }
}

pub(crate) fn load_account(conn: &rusqlite::Connection, account_id: &str) -> Result<AccountForTerminal, String> {
    conn.query_row(
        "SELECT provider, billing, config_dir_path, name, base_url, default_model FROM accounts WHERE id = ?1",
        rusqlite::params![account_id],
        |row| {
            Ok(AccountForTerminal {
                provider: row.get(0)?,
                billing: row.get(1)?,
                config_dir_path: row.get(2)?,
                name: row.get(3)?,
                endpoint: AccountEndpoint::from_columns(row.get(4)?, row.get(5)?),
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
    // A missing secret is a broken account for every provider that needs one,
    // and a normal one for opencode — which runs on its free models until a key
    // is added. Failing here would refuse to open a terminal that works.
    let key = match crate::credential_vault::get(account_id) {
        Ok(key) => Some(key),
        Err(_) if crate::providers::key_is_optional(&account.provider) => None,
        Err(e) => return Err(e),
    };
    // Which variable carries the key is the provider's business: Anthropic
    // reads `ANTHROPIC_API_KEY`, Moonshot is reached through Claude Code's
    // `ANTHROPIC_AUTH_TOKEN`, and Codex wants `OPENAI_API_KEY`.
    let var = crate::providers::spec(&account.provider).map(|s| s.key_env).unwrap_or("ANTHROPIC_API_KEY");
    let mut env: Vec<(String, String)> = key.into_iter().map(|k| (var.to_string(), k)).collect();
    for (k, v) in crate::providers::spec(&account.provider).map(|s| s.extra_env).unwrap_or(&[]) {
        env.push((k.to_string(), v.to_string()));
    }
    for (k, v) in account.endpoint.iter().flat_map(AccountEndpoint::env) {
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

/// A filesystem path, quoted for the shell that will receive it.
///
/// Separate from `shell_quote` because a path is the one kind of value that
/// needs rewriting rather than just quoting. On Windows every path Hangar holds
/// comes back from `git worktree` or the OS with backslashes, and the shell
/// these scripts run in is Git's bash: `cd 'C:\Users\me\proj'` puts bash inside
/// a directory whose name it reads as containing escape sequences, and `cd
/// C:\Users\me\proj` is worse. Bash accepts `C:/Users/me/proj` — and so does
/// every Windows API — so the separator is normalised once, here.
///
/// A no-op on Unix, where a backslash in a path is a legitimate character in a
/// filename and rewriting it would corrupt the path rather than fix it.
fn shell_path(value: &str) -> String {
    #[cfg(windows)]
    let value = &value.replace('\\', "/");
    shell_quote(value)
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
    account_command_line_with_mode(account, init, session_uuid, prompt, None)
}

/// Igual, con un modo de permisos.
///
/// Las terminales nunca han llevado modo: `Mode` existía desde el principio pero
/// solo lo usaban los turnos headless, así que el ajuste de la interfaz no
/// llegaba al CLI que de verdad trabaja. El modo global (`MOBILE_BRIDGE_TRD.md`
/// §13) es lo que lo hace necesario — sin él, una app móvil sin notificaciones
/// no puede funcionar, porque siempre habría un permiso esperando a nadie.
pub(crate) fn account_command_line_with_mode(
    account: &AccountForTerminal,
    init: InitCommand,
    session_uuid: Option<&str>,
    prompt: Option<&str>,
    mode: Option<crate::providers::Mode>,
) -> Result<String, String> {
    account_command_line_full(account, init, session_uuid, prompt, mode, None, None, None, &[], &[], None)
}

/// Everything Hangar adds to a terminal beyond the command itself: the user's
/// standing preferences and the skills this project turns on.
///
/// Grouped rather than passed as two more positional arguments because they
/// travel together, are both `Option`, and both come from the same place — the
/// project the terminal belongs to.
#[derive(Debug, Clone, Default)]
pub(crate) struct TerminalContext {
    /// Language preferences, in both shapes the CLIs want.
    pub instructions: Option<crate::commands::preferences::ResolvedInstructions>,
    /// A directory containing `.claude/skills/`, or `None` when this project
    /// has nothing selected.
    pub skills_dir: Option<std::path::PathBuf>,
    /// The `--mcp-config` file pointing at this project's code graph, or `None`
    /// when there is no graph built (the common case) or graphify isn't
    /// installed. `None` means no flag at all — never a flag pointing at a
    /// graph that doesn't exist.
    pub graph_mcp: Option<std::path::PathBuf>,
    /// Checkouts of the projects this one shares a graph group with, so an
    /// agent can open the code the graph tells it about. Empty for a project
    /// in no group, which is the common case.
    pub sibling_dirs: Vec<std::path::PathBuf>,
    /// Environment a *linked skill* needs, decided from the symlinks actually
    /// in place (`skills::linked_skills`). Two rows today: the silenced update
    /// check, and where diagrams go. Computed here, where the project id and
    /// the app data directory are both known, rather than derived from a path
    /// inside the command-line builder.
    pub skill_env: Vec<(String, String)>,
    /// This terminal's `--settings` file (`auto_continue::write_terminal_settings`):
    /// whether Claude Code waits out a usage limit here, and the status line
    /// that reports the reset time. `None` outside a claude account terminal.
    pub settings_file: Option<std::path::PathBuf>,
    /// Environment the permission mode needs when the CLI has no flag for it
    /// (`providers::permission_env`): opencode's plan agent, picked through a
    /// config file. Rendered like `skill_env`, and skipped for a login.
    pub mode_env: Vec<(String, String)>,
}

/// Igual, más el fichero de preferencias del usuario (idioma de la conversación
/// y del código).
///
/// Va aquí y no delante del prompt porque **una terminal interactiva no tiene
/// prompt**: la persona teclea contra el PTY, así que no hay primer mensaje que
/// Hangar pueda decorar. La bandera al arrancar es el único canal que existe
/// para el caso que motivó todo esto.
pub(crate) fn account_command_line_full(
    account: &AccountForTerminal,
    init: InitCommand,
    session_uuid: Option<&str>,
    prompt: Option<&str>,
    mode: Option<crate::providers::Mode>,
    instructions: Option<&crate::commands::preferences::ResolvedInstructions>,
    skills_dir: Option<&std::path::Path>,
    graph_mcp: Option<&std::path::Path>,
    sibling_dirs: &[std::path::PathBuf],
    skill_env: &[(String, String)],
    settings_file: Option<&std::path::Path>,
) -> Result<String, String> {
    let spec = crate::providers::spec(&account.provider).ok_or_else(|| format!("unknown provider '{}'", account.provider))?;
    let binary = spec.binary;
    if !crate::providers::is_provider_available(&account.provider) {
        return Err(format!("the `{binary}` CLI isn't installed on this machine"));
    }
    let config = shell_path(&account.config_dir_path);
    // Fixed environment the provider always needs — for Kimi, pointing Claude
    // Code at Moonshot's Anthropic-compatible endpoint. Rendered into the
    // command line so what the picker previews is literally what runs.
    // A custom account's endpoint goes the same way and for the same reason:
    // the address is where the project's code is about to be sent.
    let extra: String = spec
        .extra_env
        .iter()
        .copied()
        .chain(account.endpoint.iter().flat_map(AccountEndpoint::env))
        .map(|(k, v)| format!("{k}={} ", shell_quote(v)))
        .collect();
    // Y el entorno que piden las skills enlazadas: callar la llamada a casa de
    // archify, y decirle dónde dejar los diagramas. Va junto al del proveedor
    // porque es lo mismo —entorno fijo que la línea enseña— y se salta en un
    // login por la misma razón que las skills: un login no ejecuta nada del
    // proyecto, así que no hay skill en juego.
    let silenced: String = match init {
        InitCommand::Login => String::new(),
        _ => skill_env.iter().map(|(k, v)| format!("{k}={} ", shell_quote(v))).collect(),
    };
    // The trailing prompt, if any — a positional first message. Shared by Start
    // and Resume (both open an interactive session that can carry one).
    let prompt_arg = || match prompt.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => format!(" {}", shell_quote(p)),
        None => String::new(),
    };
    // Antes del prompt y después de la sesión: el orden no le importa a ningún
    // CLI, pero leerlo sí a quien mira el comando que Hangar va a ejecutar.
    let permissions: String = match (init, mode) {
        // Un login no ejecuta nada del proyecto: darle permisos no significa
        // nada y `claude auth login --permission-mode` no es un comando válido.
        (InitCommand::Login, _) => String::new(),
        (_, Some(mode)) => crate::providers::permission_args(&account.provider, mode).iter().map(|flag| format!(" {flag}")).collect(),
        (_, None) => String::new(),
    };

    // Mismo trato que los permisos: nada para un login, y nada para un
    // proveedor sin canal (Codex), donde `instruction_args` devuelve vacío y
    // esto queda en la cadena vacía.
    let instructions: String = match (init, instructions) {
        (InitCommand::Login, _) => String::new(),
        (_, Some(resolved)) => {
            crate::providers::instruction_args(&account.provider, &resolved.file.to_string_lossy(), &resolved.inline)
                .iter()
                .map(|arg| format!(" {}", shell_quote(arg)))
                .collect()
        }
        (_, None) => String::new(),
    };

    // Mismo trato que las instrucciones: un login no ejecuta nada del
    // proyecto, así que no tiene sentido darle sus skills.
    let skills: String = match (init, skills_dir) {
        (InitCommand::Login, _) => String::new(),
        (_, Some(dir)) => crate::providers::skills_args(&account.provider, &dir.to_string_lossy())
            .iter()
            .map(|arg| format!(" {}", shell_path(arg)))
            .collect(),
        (_, None) => String::new(),
    };

    // Los proyectos hermanos de un grupo de grafo (`GRAPHIFY_TRD.md` §5.6).
    // Sin esto el grafo le hablaría al agente de código que no puede abrir, que
    // es peor que no contárselo: `--add-dir` es la misma bandera que ya lleva
    // las skills, y ahí ya está documentado que además concede acceso.
    let siblings: String = match init {
        InitCommand::Login => String::new(),
        _ => sibling_dirs
            .iter()
            .flat_map(|dir| crate::providers::skills_args(&account.provider, &dir.to_string_lossy()))
            .map(|arg| format!(" {}", shell_path(&arg)))
            .collect(),
    };

    // Y lo mismo con el grafo: un login no consulta código, y sin grafo
    // construido `graph_mcp` llega en `None` y aquí no se añade nada. Ese es el
    // caso normal — la mayoría de proyectos no tendrán grafo.
    let graph: String = match (init, graph_mcp) {
        (InitCommand::Login, _) => String::new(),
        (_, Some(config)) => crate::providers::mcp_config_args(&account.provider, &config.to_string_lossy())
            .iter()
            .map(|arg| format!(" {}", shell_path(arg)))
            .collect(),
        (_, None) => String::new(),
    };

    // Claude Code only (`--settings` is its flag; Kimi runs the same binary),
    // and never for a login, which starts no session to wait in.
    let settings: String = match (init, settings_file) {
        (InitCommand::Login, _) => String::new(),
        (_, Some(file)) if binary == "claude" => format!(" --settings {}", shell_path(&file.to_string_lossy())),
        _ => String::new(),
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
        InitCommand::Resume => match (spec.session, session_uuid) {
            (crate::providers::SessionControl::MintableId { resume_flag, .. }, uuid) => {
                let uuid = uuid.ok_or("resuming a terminal needs the session id to resume")?;
                format!(" {resume_flag} {}{}", shell_quote(uuid), prompt_arg())
            }
            // Codex names its own session, so the id can't be chosen up front —
            // but it *records* it, and Hangar reads it back once the session
            // exists (see `learn_assigned_session_id`). With that id this is a
            // real resume, not a fresh start wearing the same panel.
            (crate::providers::SessionControl::AssignedId { resume_subcommand }, Some(uuid)) => {
                format!(" {resume_subcommand} {}{}", shell_quote(uuid), prompt_arg())
            }
            // Still nothing learned — the terminal never ran long enough to
            // write a session. Starting fresh is the honest outcome; resuming
            // "the most recent session" would be a guess dressed as continuity.
            _ => prompt_arg(),
        },
        // A login command takes no prompt or session — there's nothing to seed.
        InitCommand::Login => match spec.login_args {
            Some(login) => format!(" {}", login.join(" ")),
            None => return Err(format!("'{}' authenticates with an API key — there's no login to run", account.provider)),
        },
    };
    Ok(format!(
        "{}={config} {extra}{silenced}{binary}{permissions}{instructions}{skills}{siblings}{graph}{settings}{args}",
        spec.config_dir_env
    ))
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
    terminal_script_with_mode(cwd, account, init, session_uuid, prompt, None)
}

pub(crate) fn terminal_script_with_mode(
    cwd: &str,
    account: &AccountForTerminal,
    init: InitCommand,
    session_uuid: Option<&str>,
    prompt: Option<&str>,
    mode: Option<crate::providers::Mode>,
) -> Result<String, String> {
    terminal_script_full(cwd, account, init, session_uuid, prompt, mode, &TerminalContext::default())
}

pub(crate) fn terminal_script_full(
    cwd: &str,
    account: &AccountForTerminal,
    init: InitCommand,
    session_uuid: Option<&str>,
    prompt: Option<&str>,
    mode: Option<crate::providers::Mode>,
    context: &TerminalContext,
) -> Result<String, String> {
    Ok(format!(
        "cd {}\n{}",
        shell_path(cwd),
        account_command_line_full(
            account,
            init,
            session_uuid,
            prompt,
            mode,
            context.instructions.as_ref(),
            context.skills_dir.as_deref(),
            context.graph_mcp.as_deref(),
            &context.sibling_dirs,
            &[context.skill_env.as_slice(), context.mode_env.as_slice()].concat(),
            context.settings_file.as_deref(),
        )?
    ))
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
    resolve_target_with_project(conn, target).map(|(cwd, title, _)| (cwd, title))
}

/// Same, plus **which project this terminal belongs to** — the key the agent
/// preferences are stored under.
///
/// It costs nothing: the session branch already had to read `project_id` to
/// find a fallback directory, and used to throw it away.
fn resolve_target_with_project(conn: &rusqlite::Connection, target: &TerminalTarget) -> Result<(String, String, String), String> {
    match target.kind {
        TargetKind::Project => conn
            .query_row(
                "SELECT path, name FROM projects WHERE id = ?1",
                rusqlite::params![target.id],
                |row| Ok((row.get(0)?, row.get(1)?, target.id.clone())),
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
            Ok((cwd, name, project_id))
        }
    }
}

/// Re-authenticates one account, in a real terminal the user can watch.
///
/// `InitCommand::Login` has been able to do this all along — it builds
/// `<CONFIG_DIR_ENV>=<the account's own dir> <cli> auth login`, which is the
/// only path that recovers an expired account without deleting and re-adding
/// it. What was missing was any way to ask for it: `open_terminal` takes an
/// `init`, and every caller passed `None`.
///
/// The working directory is incidental here — a login talks to the provider,
/// not to a repo — so it takes any project rather than making the user choose
/// one for a question that doesn't depend on the answer. The account's config
/// dir, which *is* what matters, comes from the account row either way.
#[tauri::command]
pub async fn open_login_terminal<R: Runtime>(app: AppHandle<R>, account_id: String, cols: u16, rows: u16) -> Result<TerminalInfo, String> {
    crate::blocking::run_blocking(move || {
        let project_id: String = {
            let db = app.state::<DbConnection>();
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT id FROM projects ORDER BY created_at ASC LIMIT 1", [], |row| row.get(0))
                .map_err(|_| "add a project first — a terminal has to open somewhere, even to log in".to_string())?
        };
        open_terminal_blocking(
            app,
            TerminalTarget { kind: TargetKind::Project, id: project_id },
            Some(account_id),
            Some(InitCommand::Login),
            None,
            None,
            cols,
            rows,
        )
    })
    .await?
}

/// Opens a terminal from the frontend.
///
/// A thin `async` shell over `open_terminal_blocking` so the work lands on the
/// blocking pool: spawning a PTY means SQLite lookups, a shell process and
/// `git` for a session's worktree, and a synchronous command would do all of
/// that on the thread that draws the window (see `crate::blocking`). Callers
/// inside Rust use the blocking form directly — they are already off that
/// thread, and several of them open terminals in a loop.
#[tauri::command]
pub async fn open_terminal<R: Runtime>(
    app: AppHandle<R>,
    target: TerminalTarget,
    account_id: Option<String>,
    init: Option<InitCommand>,
    session_uuid: Option<String>,
    init_prompt: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<TerminalInfo, String> {
    crate::blocking::run_blocking(move || open_terminal_blocking(app, target, account_id, init, session_uuid, init_prompt, cols, rows)).await?
}

pub(crate) fn open_terminal_blocking<R: Runtime>(
    app: AppHandle<R>,
    target: TerminalTarget,
    account_id: Option<String>,
    init: Option<InitCommand>,
    session_uuid: Option<String>,
    init_prompt: Option<String>,
    cols: u16,
    rows: u16,
) -> Result<TerminalInfo, String> {
    open_terminal_with(app, target, account_id, init, session_uuid, init_prompt, cols, rows, false)
}

/// `open_terminal_blocking` plus the per-terminal auto-continue choice, which
/// only a restore and a relaunch have to carry: a new terminal starts with it
/// off.
#[allow(clippy::too_many_arguments)]
fn open_terminal_with<R: Runtime>(
    app: AppHandle<R>,
    target: TerminalTarget,
    account_id: Option<String>,
    init: Option<InitCommand>,
    session_uuid: Option<String>,
    init_prompt: Option<String>,
    cols: u16,
    rows: u16,
    auto_continue: bool,
) -> Result<TerminalInfo, String> {
    let (cwd, title, account, mut context) = {
        let db = app.state::<DbConnection>();
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let (cwd, title, project_id) = resolve_target_with_project(&conn, &target)?;
        let account = match &account_id {
            Some(id) => Some(load_account(&conn, id)?),
            None => None,
        };
        // Best-effort, both of them: a terminal that opens without the user's
        // language preference or without its skills is a worse terminal, but
        // one that refuses to open because a file couldn't be written is a
        // broken app.
        let root = crate::app_data::root(&app);
        let instructions = root
            .as_ref()
            .map_err(|e| e.clone())
            .and_then(|root| crate::commands::preferences::instruction_file(root, &conn, Some(&project_id)))
            .unwrap_or_else(|e| {
                log::warn!("open_terminal: could not prepare the agent preferences — {e}");
                None
            });
        let skills_dir = root
            .as_ref()
            .map_err(|e| e.clone())
            .and_then(|root| crate::commands::skills::skills_dir(root, &conn, &project_id))
            .unwrap_or_else(|e| {
                log::warn!("open_terminal: could not prepare this project's skills — {e}");
                None
            });
        // Las skills «siempre activas» van por otro camino: se enlazan en el
        // config dir de esta cuenta, donde cada CLI ya mira por su cuenta. Por
        // eso no añaden ninguna bandera y llegan también a los proveedores cuyas
        // banderas no están verificadas.
        if let (Ok(root), Some(account)) = (root.as_ref(), account.as_ref()) {
            if let Err(e) = crate::commands::skills::link_always_on(root, &conn, std::path::Path::new(&account.config_dir_path)) {
                log::warn!("open_terminal: could not link the always-on skills — {e}");
            }
        }
        // Mismo trato de «mejor esfuerzo»: sin grafo construido esto es `None`
        // y la terminal abre exactamente como hoy. Y tiene que ser `None` en
        // ese caso — registrar un MCP que apunta a un grafo inexistente le da
        // al agente una herramienta que falla al usarla (§5.5).
        let graph_mcp = crate::commands::graph::mcp_config_for_project(&app, &conn, &project_id);
        let sibling_dirs = crate::commands::graph::sibling_dirs(&conn, &project_id);
        // Lo que piden las skills que de verdad quedaron enlazadas. Se decide
        // aquí, que es donde se conocen el id del proyecto y el directorio de
        // datos, y no dentro del constructor de la línea de comandos — que solo
        // tiene una ruta y tendría que hacer aritmética con ella para adivinar
        // los dos.
        let skill_env: Vec<(String, String)> = match (root.as_ref(), skills_dir.as_deref()) {
            (Ok(root), Some(dir)) => {
                let linked = crate::commands::skills::linked_skills(dir);
                let mut env = crate::commands::skills::silencing_env(&linked);
                env.extend(crate::commands::skills::diagrams_env(&linked, root, &project_id));
                env
            }
            _ => Vec::new(),
        };
        (cwd, title, account, TerminalContext { instructions, skills_dir, graph_mcp, sibling_dirs, skill_env, settings_file: None, mode_env: Vec::new() })
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

    // Minted before the command line, because the command names this
    // terminal's own `--settings` file.
    let id = Uuid::new_v4().to_string();
    let status_root = crate::app_data::root(&app).ok();
    if let (Some(account), Some(InitCommand::Start | InitCommand::Resume)) = (&account, init) {
        let is_claude = crate::providers::spec(&account.provider).is_some_and(|s| s.binary == "claude");
        if let (true, Some(root)) = (is_claude, status_root.as_ref()) {
            match crate::auto_continue::write_terminal_settings(root, &id, auto_continue, std::path::Path::new(&account.config_dir_path)) {
                Ok(path) => context.settings_file = Some(path),
                // Without it Claude Code falls back to its own default (wait
                // on) — worth a log line, not a terminal that won't open.
                Err(e) => log::warn!("open_terminal: could not write the terminal's settings — {e}"),
            }
        }
    }

    // Resolve the command before spawning anything: a missing CLI should
    // surface as an error the user can act on, not as a terminal that opens
    // and greets them with `command not found`.
    let init_line = match (&account, init) {
        (Some(account), Some(init)) => {
            // El modo global de la máquina, aplicado al nacer la terminal. No se
            // puede cambiar después sin teclearle al CLI a ciegas, así que el
            // ajuste dice explícitamente que afecta a las que se abran.
            let mode = app.try_state::<DbConnection>().map(|db| crate::bridge::session::default_mode(&db));
            // opencode 2.x has no flag for its plan agent; it's picked through
            // a config file Hangar keeps next to its other per-app files.
            if let (Some(crate::providers::Mode::Plan), "opencode", Some(root)) = (mode, account.provider.as_str(), status_root.as_ref()) {
                let file = root.join("opencode").join("plan.json");
                let written = std::fs::create_dir_all(root.join("opencode")).and_then(|()| std::fs::write(&file, crate::providers::OPENCODE_PLAN_CONFIG));
                match written {
                    Ok(()) => context.mode_env = crate::providers::permission_env("opencode", crate::providers::Mode::Plan, Some(&file)),
                    // Opens in opencode's default agent, which asks before
                    // acting — a terminal that starts beats one that doesn't.
                    Err(e) => log::warn!("open_terminal: could not write opencode's plan config — {e}"),
                }
            }
            Some(format!(
                "{}\n",
                terminal_script_full(&cwd, account, init, session_uuid.as_deref(), init_prompt.as_deref(), mode, &context)?
            ))
        }
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
    // Where the status line wrapper files this account's reset times.
    if let Some(id) = &account_id {
        account_vars.push((crate::auto_continue::STATUS_KEY_ENV.to_string(), id.clone()));
    }
    let account_name = account.as_ref().map(|a| a.name.clone());

    let pty = native_pty_system();
    let pair = pty
        // A zero dimension makes some shells misbehave before the frontend's
        // first real `fit()` lands, so clamp to something sane — and see
        // `MIN_PTY_COLS` for why "sane" is not 1 column.
        .openpty(PtySize { rows: rows.max(1), cols: cols.max(MIN_PTY_COLS), pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;

    let mut cmd = CommandBuilder::new(user_shell());
    // `-l` is a POSIX login shell's flag. PowerShell reads it as a positional
    // argument — a script file named `-l` — and exits immediately, which would
    // look like the terminal closing the moment it opens.
    if shell_is_posix() {
        cmd.arg("-l");
    } else {
        cmd.arg("-NoLogo");
    }
    cmd.cwd(&cwd);
    for (k, v) in terminal_env(&account_vars, account.is_some()) {
        cmd.env(k, v);
    }

    let child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
    // Un hilo dueño del writer, alimentado por un canal. Que el orden lo fije el
    // canal y no la carrera por un mutex es la mitad del arreglo: el frontend
    // invoca `write_terminal` sin esperar (`TerminalPane.tsx:115`), así que dos
    // pulsaciones que compitieran por un candado podrían llegar cambiadas.
    let (writes, incoming) = std::sync::mpsc::sync_channel::<Vec<u8>>(WRITE_QUEUE_DEPTH);

    {
        let id = id.clone();
        std::thread::spawn(move || {
            // Termina solo: cuando el handle se tira, el emisor muere y `recv`
            // devuelve Err.
            while let Ok(chunk) = incoming.recv() {
                if let Err(e) = writer.write_all(&chunk).and_then(|()| writer.flush()) {
                    log::warn!("terminal {id}: could not write to the pty: {e}");
                    break;
                }
            }
        });
    }
    let info = TerminalInfo {
        // A terminal the user just asked for is never born folded; restore sets
        // it afterwards for the ones that were.
        minimized: false,
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
        auto_continue,
    };

    let scrollback = Arc::new(Mutex::new(VecDeque::new()));
    let sink: Arc<Mutex<Option<TerminalSink>>> = Arc::new(Mutex::new(None));
    let activity = Arc::new(Mutex::new(Activity::new()));
    let mobile_tail = Arc::new(Mutex::new(crate::bridge::redact::PtyTail::new(rows.max(1), cols.max(1))));
    let auto = Arc::new(Mutex::new(AutoContinue { enabled: auto_continue, ..AutoContinue::default() }));

    {
        let scrollback = scrollback.clone();
        let sink = sink.clone();
        let activity = activity.clone();
        let mobile_tail = mobile_tail.clone();
        let auto = auto.clone();
        let status_root = status_root.clone();
        let status_account = account_id.clone();
        let id = id.clone();
        let app_for_reader = app.clone();
        std::thread::spawn(move || {
            let mut buf = vec![0u8; READ_CHUNK_BYTES];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let chunk = &buf[..n];
                        // The sink is taken *before* the scrollback and held
                        // across both, which is the order `attach_terminal`
                        // takes them in. It has to be: attach snapshots the
                        // scrollback and installs an empty sink as one step, so
                        // a reader that could slip between those two would put
                        // this chunk in the snapshot *and* in the new sink's
                        // queue, and the pane would print it twice. Same order
                        // on both sides means one of them simply waits.
                        let mut sink_guard = sink.lock();
                        if let Ok(mut buffer) = scrollback.lock() {
                            push_scrollback(&mut buffer, chunk);
                        }
                        // Queued, not sent: `tick_terminal_output` does the
                        // sending, one message per tick per terminal, so a
                        // program printing fast can't flood the UI thread.
                        if let Ok(sink) = sink_guard.as_mut() {
                            if let Some(sink) = sink.as_mut() {
                                sink.pending.extend_from_slice(chunk);
                                if sink.pending.len() > PENDING_OUTPUT_LIMIT_BYTES {
                                    let excess = sink.pending.len() - PENDING_OUTPUT_LIMIT_BYTES;
                                    sink.pending.drain(..excess);
                                    log::warn!("terminal {id}: dropped {excess} bytes the pane couldn't keep up with");
                                }
                            }
                        }
                        drop(sink_guard);
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
                        // Redacted mobile tail — see `bridge::redact`. Fed
                        // unconditionally; `tick_mobile_tail` is what actually
                        // throttles the emit, this just keeps the buffer current.
                        if let Ok(mut tail) = mobile_tail.lock() {
                            tail.ingest(chunk);
                        }
                        // Claude Code saying it won't wait out this limit — one of
                        // the two gaps Hangar covers, and only where the user asked.
                        if let Ok(mut auto) = auto.lock() {
                            if auto.enabled {
                                auto.recent.push_str(&crate::auto_continue::plain_text(chunk));
                                if auto.recent.len() > 4096 {
                                    let keep_from = auto.recent.len() - 2048;
                                    let start = auto.recent.char_indices().map(|(i, _)| i).find(|&i| i >= keep_from).unwrap_or(0);
                                    auto.recent = auto.recent[start..].to_string();
                                }
                                if crate::auto_continue::native_gave_up(&auto.recent) {
                                    auto.recent.clear();
                                    let (at, estimated) = arm_time(status_root.as_deref(), status_account.as_deref());
                                    auto.resume_at = Some(at);
                                    auto.estimated = estimated;
                                    log::info!("terminal {id}: Claude Code won't wait out this limit — Hangar continues it at {at}");
                                    emit_auto(&app_for_reader, &id, &auto);
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            if let Ok(mut act) = activity.lock() {
                act.state = TerminalState::Exited;
            }
            if let Ok(mut tail) = mobile_tail.lock() {
                tail.finish();
                if let Some(text) = tail.take_pending() {
                    emit_tail(&app_for_reader, &id, text);
                }
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
        // Por el mismo canal que las teclas, y eso arregla de paso una carrera
        // que ya existía: los dos competían por el mutex del writer, así que
        // teclear rápido podía colar tu pulsación por delante del guion.
        let writes = writes.clone();
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

            if let Err(e) = writes.try_send(line.into_bytes()) {
                log::warn!("terminal {id}: could not type the init command: {e}");
            }
        });
    }

    app.state::<TerminalRegistry>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .insert(id.clone(), TerminalHandle { master: pair.master, writes, child, scrollback, sink, activity, mobile_tail, auto, info: info.clone() });

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
        "INSERT OR REPLACE INTO open_terminals (id, target_kind, source_id, account_id, session_uuid, display_name, minimized, auto_continue, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![info.id, kind, info.source_id, info.account_id, info.session_uuid, info.display_name, info.minimized, info.auto_continue, chrono::Utc::now().to_rfc3339()],
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
    // Nothing pending: the snapshot above already carries everything printed so
    // far, and this is the one place where "so far" is unambiguous.
    *sink = Some(TerminalSink { channel, pending: Vec::new() });

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

/// Encola lo tecleado; el hilo escritor de esa terminal lo saca.
///
/// **Sigue siendo síncrono a propósito.** Encolar es instantáneo, y que la IPC
/// despache en serie es lo que garantiza que las pulsaciones lleguen en orden:
/// el frontend invoca esto sin esperar la respuesta, así que repartir cada
/// escritura a un hilo del pool las pondría a competir y «ls» podría salir
/// «sl».
///
/// Lo que ya no hace es escribir al PTY con el candado global puesto. Esa era
/// la forma que congelaba la app entera cuando el hijo dejaba de leer.
#[tauri::command]
pub fn write_terminal<R: Runtime>(app: AppHandle<R>, id: String, data: String) -> Result<(), String> {
    let (writes, auto) = {
        let registry = app.state::<TerminalRegistry>();
        let map = registry.0.lock().map_err(|e| e.to_string())?;
        let handle = map.get(&id).ok_or_else(|| format!("no terminal with id {id}"))?;
        (handle.writes.clone(), handle.auto.clone())
    };
    // The user took over: whatever they typed is the next move, not ours.
    if crate::auto_continue::is_user_input(&data) {
        if let Ok(mut auto) = auto.lock() {
            if auto.resume_at.take().is_some() {
                log::info!("terminal {id}: auto-continue cancelled — the user typed first");
                emit_auto(&app, &id, &auto);
            }
        }
    }
    match writes.try_send(data.into_bytes()) {
        Ok(()) => Ok(()),
        Err(std::sync::mpsc::TrySendError::Full(_)) => {
            Err(format!("terminal {id} is not reading its input — {WRITE_QUEUE_DEPTH} writes are already waiting"))
        }
        Err(std::sync::mpsc::TrySendError::Disconnected(_)) => Err(format!("terminal {id} is no longer running")),
    }
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
        // Same floor as the open: a resize down to a few columns kills the
        // same CLIs a narrow open does.
        .resize(PtySize { rows: rows.max(1), cols: cols.max(MIN_PTY_COLS), pixel_width: 0, pixel_height: 0 })
        .map_err(|e| e.to_string())?;
    // Keeps the mobile tail's vt100 buffer at the same geometry as the real
    // PTY — its cursor math (and Ink's own hard-wrap decisions) are relative
    // to this size, so letting them drift apart would misinterpret every
    // cursor-movement byte that follows.
    if let Ok(mut tail) = handle.mobile_tail.lock() {
        tail.resize(rows.max(1), cols.max(1));
    }
    Ok(())
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
    if let Ok(root) = crate::app_data::root(&app) {
        crate::auto_continue::forget_terminal(&root, &id);
    }
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

/// The branch each open terminal's folder is on, by terminal id — what lets
/// the Tasks panel say "this task already has a terminal" (a task's worktree is
/// on its suggested branch). Asked of git rather than remembered: the agent can
/// switch branches, and a stale answer would send you to the wrong panel.
/// Terminals outside a repo, or on a detached HEAD, are left out.
#[tauri::command]
pub async fn terminal_branches<R: Runtime>(app: AppHandle<R>) -> Result<HashMap<String, String>, String> {
    let dirs: Vec<(String, String)> = {
        let registry = app.state::<TerminalRegistry>();
        let map = registry.0.lock().map_err(|e| e.to_string())?;
        map.values().map(|h| (h.info.id.clone(), h.info.cwd.clone())).collect()
    };
    crate::blocking::run_blocking(move || {
        dirs.into_iter()
            .filter_map(|(id, cwd)| {
                let out = crate::providers::git_command().args(["-C", &cwd, "branch", "--show-current"]).output().ok()?;
                let branch = String::from_utf8_lossy(&out.stdout).trim().to_string();
                (out.status.success() && !branch.is_empty()).then_some((id, branch))
            })
            .collect()
    })
    .await
}

struct PersistedTerminal {
    target_kind: String,
    source_id: String,
    account_id: Option<String>,
    session_uuid: Option<String>,
    display_name: Option<String>,
    minimized: bool,
    auto_continue: bool,
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
/// Off the UI thread because it reopens *every* terminal the last session had
/// open, one after another — each with its own PTY spawn and SQLite lookups.
/// As a synchronous command this froze the window for the whole restore.
#[tauri::command]
pub async fn restore_terminals<R: Runtime>(app: AppHandle<R>, cols: u16, rows: u16) -> Result<Vec<TerminalInfo>, String> {
    crate::blocking::run_blocking(move || restore_terminals_blocking(app, cols, rows)).await?
}

fn restore_terminals_blocking<R: Runtime>(app: AppHandle<R>, cols: u16, rows: u16) -> Result<Vec<TerminalInfo>, String> {
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
            .prepare("SELECT target_kind, source_id, account_id, session_uuid, display_name, minimized, auto_continue FROM open_terminals ORDER BY created_at ASC")
            .map_err(|e| e.to_string())?;
        let collected: Vec<PersistedTerminal> = stmt
            .query_map([], |r| {
                Ok(PersistedTerminal {
                    target_kind: r.get(0)?,
                    source_id: r.get(1)?,
                    account_id: r.get(2)?,
                    session_uuid: r.get(3)?,
                    display_name: r.get(4)?,
                    minimized: r.get::<_, i64>(5)? != 0,
                    auto_continue: r.get::<_, i64>(6)? != 0,
                })
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
        // The choice comes back with the panel: a restart is exactly when Claude
        // Code's own wait is lost, so this is when it matters most.
        match open_terminal_with(app.clone(), target, row.account_id.clone(), init, session_uuid, None, cols, rows, row.auto_continue) {
            Ok(mut info) => {
                // The name the user gave this panel is part of what "restore"
                // means — without it, three panels on one project come back
                // indistinguishable.
                if let Some(name) = row.display_name.clone() {
                    let _ = rename_terminal(app.clone(), info.id.clone(), name.clone());
                    info.display_name = Some(name);
                }
                // Same reasoning as the name: a panel the user folded comes back
                // folded, or "restore" has quietly rearranged their screen.
                if row.minimized {
                    let _ = set_terminal_minimized(app.clone(), info.id.clone(), true);
                    info.minimized = true;
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
            if session_is_resumable(app, account_id, &config_dir, uuid) {
                return (Some(InitCommand::Resume), Some(uuid.clone()));
            }
        }
    }
    // Account known but nothing to resume — start a fresh session under it.
    (Some(InitCommand::Start), None)
}

/// Whether the CLI still has the conversation this id names.
///
/// Per provider, because "where the transcript lives" is per CLI: Claude keeps
/// one file per session id under its config dir, Codex keeps rollouts under
/// `sessions/`. Checking Claude's layout for a Codex id — which is what this
/// used to do — always answers no, so a Codex terminal always came back as a
/// fresh session even once its id was known.
fn session_is_resumable<R: Runtime>(app: &AppHandle<R>, account_id: &str, config_dir: &str, session_uuid: &str) -> bool {
    let provider = app
        .try_state::<DbConnection>()
        .and_then(|db| {
            db.0.lock()
                .ok()
                .and_then(|conn| conn.query_row("SELECT provider FROM accounts WHERE id = ?1", rusqlite::params![account_id], |r| r.get::<_, String>(0)).ok())
        })
        .unwrap_or_else(|| "claude".to_string());

    match crate::providers::spec(&provider).map(|s| s.session) {
        Some(crate::providers::SessionControl::AssignedId { .. }) => crate::usage_meter::rollout_exists(config_dir, session_uuid),
        _ => crate::handoff::transcript_exists(config_dir, session_uuid),
    }
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
    if !carries_conversation(&from.provider, &to.provider) {
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

/// Whether a terminal moving between these two providers can take its
/// transcript along as a file copy.
///
/// Claude→Claude is the verified case (`handoff`). A `custom` account is the
/// same binary reading the same transcript format, so moving *onto* one — or
/// between two — is the same copy.
///
/// ASSUMPTION, not verified against a real endpoint: a transcript written by
/// Claude resumes on a third-party endpoint, on the grounds that those ignore
/// the signatures on thinking blocks. The reverse is deliberately refused:
/// Anthropic validates those signatures, and a transcript holding blocks another
/// model produced is the one that could make every later turn fail. Coming back
/// from a fallback therefore starts fresh on the same worktree. Tighten or
/// loosen both once this has been run against a real key.
fn carries_conversation(from_provider: &str, to_provider: &str) -> bool {
    matches!((from_provider, to_provider), ("claude", "claude") | ("claude", "custom") | ("custom", "custom"))
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
    let info = open_terminal_blocking(app.clone(), target, Some(account_id.clone()), init, session_uuid, None, cols, rows)?;
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
        match open_terminal_blocking(
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
/// Folds or unfolds a panel, and remembers it.
///
/// Same shape as `rename_terminal` below, for the same reason: the registry
/// entry is what the running app reads and the row is what a restart reads, so
/// both have to move together or the two disagree the moment Hangar reopens.
#[tauri::command]
pub fn set_terminal_minimized<R: Runtime>(app: AppHandle<R>, id: String, minimized: bool) -> Result<(), String> {
    {
        let registry = app.state::<TerminalRegistry>();
        let mut map = registry.0.lock().map_err(|e| e.to_string())?;
        // Not an error when the terminal is gone: a panel can be folded and
        // closed in the same breath, and failing here would surface a toast for
        // something the user already finished doing.
        if let Some(handle) = map.get_mut(&id) {
            handle.info.minimized = minimized;
        }
    }
    let Some(db) = app.try_state::<DbConnection>() else { return Ok(()) };
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("UPDATE open_terminals SET minimized = ?2 WHERE id = ?1", rusqlite::params![id, minimized])
        .map(|_| ())
        .map_err(|e| e.to_string())
}

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

/// The redacted-tail equivalent of `emit_state`, for a terminal without a
/// session — see `bridge::redact`. Mobile-only: the desktop's own
/// `TerminalPane.tsx` already gets the real bytes via `attach_terminal`'s
/// `Channel`, it has no use for a redacted copy of its own screen.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MobileTailChunk {
    terminal_id: String,
    text: String,
}

fn emit_tail<R: Runtime>(app: &AppHandle<R>, id: &str, text: String) {
    let _ = app.emit("terminal-tail", MobileTailChunk { terminal_id: id.to_string(), text });
}

/// The current redacted tail of a terminal without a session, for a mobile
/// screen that just opened — mirrors `get_session_view` for a session-backed
/// terminal. Not history: whatever's still in the bounded ring (see
/// `bridge::redact::PtyTail`), same "no reproducción de eventos perdidos"
/// spirit as `MOBILE_BRIDGE_TRD.md` §9.
#[tauri::command]
pub fn get_terminal_tail<R: Runtime>(app: AppHandle<R>, id: String) -> Result<String, String> {
    let registry = app.state::<TerminalRegistry>();
    let map = registry.0.lock().map_err(|e| e.to_string())?;
    let handle = map.get(&id).ok_or_else(|| format!("no hay una terminal abierta con id {id}"))?;
    let mut tail = handle.mobile_tail.lock().map_err(|e| e.to_string())?;
    Ok(tail.snapshot())
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
    let mut to_relaunch: Vec<String> = Vec::new();
    {
        let Ok(map) = registry.0.lock() else { return };
        for (id, handle) in map.iter() {
            let Ok(mut act) = handle.activity.lock() else { continue };
            if act.state == TerminalState::Working && act.last_output.elapsed() >= QUIET_BEFORE_WAITING {
                act.state = TerminalState::Waiting;
                newly_waiting.push((id.clone(), handle.info.title.clone(), act.title.clone(), handle.info.clone()));
                if handle.auto.lock().map(|a| a.pending_relaunch).unwrap_or(false) {
                    to_relaunch.push(id.clone());
                }
            }
        }
    }

    // An auto-continue change made mid-turn, applied now that the turn is over.
    for id in to_relaunch {
        let app = app.clone();
        std::thread::spawn(move || match relaunch_terminal(&app, &id) {
            Ok(info) => {
                let _ = app.emit("terminal-replaced", TerminalReplaced { old_id: id, info });
            }
            Err(e) => log::warn!("terminal {id}: could not relaunch with the new auto-continue setting: {e}"),
        });
    }

    for (id, panel_title, agent_title, info) in newly_waiting {
        // The turn is over, so the transcript is complete — the one moment
        // worth reading it. Polling for this would mean re-reading every open
        // terminal's file on a timer to catch a change that happens a few times
        // an hour.
        record_terminal_usage(app, &info);
        learn_assigned_session_id(app, &id, &info);
        emit_state(app, &id, TerminalState::Waiting, agent_title.clone());
        // A queued run finishes when its agent stops talking — the only
        // completion signal available from outside the CLI.
        crate::run_queue::note_terminal_waiting(app, &id);
        crate::notifications::record(
            app,
            crate::notifications::kind::AGENT,
            |s| s.notify_session_waiting,
            "Agent is waiting",
            &agent_title.unwrap_or_else(|| format!("{panel_title} finished its turn")),
            crate::notifications::Target::new("terminal", id.clone()),
        );
    }
}

/// A terminal the backend swapped for a new one, so the pane can follow it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalReplaced {
    pub old_id: String,
    pub info: TerminalInfo,
}

/// Types the continuation into every terminal whose armed time has come.
///
/// Only when the agent isn't mid-output: if something else already got it
/// going again — the user, or Claude Code after all — a prompt typed into a
/// running turn would be noise at best.
pub fn tick_auto_continue<R: Runtime>(app: &AppHandle<R>) {
    let Some(registry) = app.try_state::<TerminalRegistry>() else { return };
    let Ok(map) = registry.0.lock() else { return };
    let now = chrono::Utc::now();
    for (id, handle) in map.iter() {
        let Ok(mut auto) = handle.auto.lock() else { continue };
        let due = auto.enabled && auto.resume_at.is_some_and(|at| at <= now);
        if !due {
            continue;
        }
        let working = handle.activity.lock().map(|a| a.state == TerminalState::Working).unwrap_or(true);
        if working {
            continue;
        }
        auto.resume_at = None;
        match handle.writes.try_send(format!("{}\r", crate::auto_continue::CONTINUE_PROMPT).into_bytes()) {
            Ok(()) => log::info!("terminal {id}: usage limit reset — typed the continuation"),
            Err(e) => log::warn!("terminal {id}: could not type the continuation: {e}"),
        }
        emit_auto(app, id, &auto);
    }
}

/// Every terminal's auto-continue state, for the first render; after that the
/// panes follow `terminal-auto-continue`.
#[tauri::command]
pub fn list_auto_continue<R: Runtime>(app: AppHandle<R>) -> Result<Vec<AutoContinueStatus>, String> {
    let registry = app.state::<TerminalRegistry>();
    let map = registry.0.lock().map_err(|e| e.to_string())?;
    Ok(map.iter().filter_map(|(id, h)| h.auto.lock().ok().map(|a| auto_status(id, &a))).collect())
}

/// Turns auto-continue on or off for one terminal, and returns the terminal to
/// show — a new one when the CLI had to be relaunched.
///
/// Claude Code reads `--settings` only at startup, so for a claude terminal the
/// change means relaunching it on the same conversation (`--resume`). Not in
/// the middle of a turn, though: that would cut the agent off, so it's marked
/// pending and `tick_terminal_states` does it when the turn ends.
#[tauri::command]
pub async fn set_terminal_auto_continue<R: Runtime>(app: AppHandle<R>, id: String, enabled: bool) -> Result<TerminalInfo, String> {
    crate::blocking::run_blocking(move || set_terminal_auto_continue_blocking(&app, &id, enabled)).await?
}

fn set_terminal_auto_continue_blocking<R: Runtime>(app: &AppHandle<R>, id: &str, enabled: bool) -> Result<TerminalInfo, String> {
    let (info, relaunch_now) = {
        let registry = app.state::<TerminalRegistry>();
        let mut map = registry.0.lock().map_err(|e| e.to_string())?;
        let handle = map.get_mut(id).ok_or_else(|| format!("no terminal with id {id}"))?;
        handle.info.auto_continue = enabled;
        let runs_claude = handle.info.session_uuid.is_some() && handle.info.account_id.is_some();
        let working = handle.activity.lock().map(|a| a.state == TerminalState::Working).unwrap_or(false);
        let mut auto = handle.auto.lock().map_err(|e| e.to_string())?;
        auto.enabled = enabled;
        if !enabled {
            auto.resume_at = None;
        }
        auto.pending_relaunch = runs_claude && working;
        emit_auto(app, id, &auto);
        (handle.info.clone(), runs_claude && !working)
    };
    persist_terminal(app, &info);
    if relaunch_now {
        relaunch_terminal(app, id)
    } else {
        Ok(info)
    }
}

/// Reopens a terminal's CLI on the same conversation with its current
/// auto-continue setting, keeping the panel's name and fold. Resumes when the
/// transcript is there and starts fresh otherwise — the same decision a restore
/// makes, for the same reasons.
fn relaunch_terminal<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<TerminalInfo, String> {
    let old = {
        let registry = app.state::<TerminalRegistry>();
        let map = registry.0.lock().map_err(|e| e.to_string())?;
        map.get(id).ok_or_else(|| format!("no terminal with id {id}"))?.info.clone()
    };
    let row = PersistedTerminal {
        target_kind: match old.kind {
            TargetKind::Project => "project".to_string(),
            TargetKind::Session => "session".to_string(),
        },
        source_id: old.source_id.clone(),
        account_id: old.account_id.clone(),
        session_uuid: old.session_uuid.clone(),
        display_name: old.display_name.clone(),
        minimized: old.minimized,
        auto_continue: old.auto_continue,
    };
    let (init, session_uuid) = decide_restore_init(app, &row);
    let target = TerminalTarget { kind: old.kind, id: old.source_id.clone() };
    // Open before closing, like an account switch: if the new one can't start,
    // the user keeps the one they had.
    let mut info = open_terminal_with(app.clone(), target, old.account_id.clone(), init, session_uuid, None, 80, 24, old.auto_continue)?;
    close_terminal(app.clone(), id.to_string())?;
    if let Some(name) = old.display_name {
        let _ = rename_terminal(app.clone(), info.id.clone(), name.clone());
        info.display_name = Some(name);
    }
    if old.minimized {
        let _ = set_terminal_minimized(app.clone(), info.id.clone(), true);
        info.minimized = true;
    }
    Ok(info)
}

/// Flushes any redacted-tail text that's piled up past its debounce or size
/// cap (see `bridge::redact::PtyTail::due`) as `terminal-tail` events.
/// Piggybacks on the same 1 Hz tick as `tick_terminal_states` rather than a
/// second timer thread — a phone glance doesn't need sub-second freshness.
pub fn tick_mobile_tail<R: Runtime>(app: &AppHandle<R>) {
    let Some(registry) = app.try_state::<TerminalRegistry>() else { return };
    let due: Vec<(String, String)> = {
        let Ok(map) = registry.0.lock() else { return };
        map.iter()
            .filter_map(|(id, handle)| {
                let mut tail = handle.mobile_tail.lock().ok()?;
                if !tail.due() {
                    return None;
                }
                tail.take_pending().map(|text| (id.clone(), text))
            })
            .collect()
    };
    for (id, text) in due {
        emit_tail(app, &id, text);
    }
}

/// Hands each attached pane whatever its PTY has printed since the last tick,
/// in one message per terminal — see `TerminalSink` for why the reader thread
/// no longer sends directly.
///
/// The bytes are taken out from under the lock and encoded outside it: base64
/// of half a megabyte is real work, and holding the sink while doing it would
/// stall the reader thread that wants to append the next chunk.
pub fn tick_terminal_output<R: Runtime>(app: &AppHandle<R>) {
    let Some(registry) = app.try_state::<TerminalRegistry>() else { return };
    let due: Vec<(Channel<TerminalChunk>, Vec<u8>)> = {
        let Ok(map) = registry.0.lock() else { return };
        map.values()
            .filter_map(|handle| {
                let mut sink = handle.sink.lock().ok()?;
                let sink = sink.as_mut()?;
                if sink.pending.is_empty() {
                    return None;
                }
                Some((sink.channel.clone(), std::mem::take(&mut sink.pending)))
            })
            .collect()
    };
    for (channel, bytes) in due {
        let _ = channel.send(TerminalChunk { base64: base64::engine::general_purpose::STANDARD.encode(bytes) });
    }
}

/// Reads every account's quota now, instead of waiting for the poller.
///
/// The poller only samples accounts that have a terminal open, every five
/// minutes — which is right for keeping banners fresh and wrong for a screen
/// you opened *to see the numbers*: three idle accounts show "no reading"
/// forever. This is the explicit version, and it's a command rather than
/// something the modal does on open because each Claude account costs a `claude`
/// process in a PTY (~6s), and three of those firing silently would look like
/// the app had hung.
#[tauri::command]
pub async fn refresh_all_quotas<R: Runtime>(app: AppHandle<R>) -> Result<usize, String> {
    crate::blocking::run_blocking(move || {
        let Some(db) = app.try_state::<DbConnection>() else { return 0 };
        let accounts: Vec<(String, String, String)> = match db.0.lock() {
            Ok(conn) => conn
                .prepare("SELECT id, provider, config_dir_path FROM accounts WHERE billing = 'sub'")
                .and_then(|mut stmt| {
                    stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                        .map(|rows| rows.filter_map(Result::ok).collect::<Vec<_>>())
                })
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };

        let mut read = 0;
        for (account_id, provider, config_dir) in accounts {
            match provider.as_str() {
                // Codex records the API's rate-limit headers on every turn, so
                // its quota is already on disk — no process to spawn.
                "codex" => {
                    if let Some((session, weekly)) = crate::usage_meter::codex_quota(&config_dir) {
                        if let Ok(conn) = db.0.lock() {
                            if crate::quota::record_sample(&conn, &account_id, session, weekly, None).is_ok() {
                                read += 1;
                            }
                        }
                    }
                }
                _ => {
                    if refresh_account_status(&app, &account_id) {
                        read += 1;
                    } else {
                        // `refresh_account_status` only reports *changes*, and a
                        // reading that matched the last one is still a reading.
                        read += usize::from(std::path::Path::new(&config_dir).is_dir());
                    }
                }
            }
        }
        read
    })
    .await
}

/// Learns the session id a CLI chose for itself, so the terminal can be
/// resumed later.
///
/// Only for providers that assign their own — Claude's is minted by Hangar and
/// known before the process starts. Codex has no flag to name a session, which
/// is why its terminals came back empty after a relaunch: there was no id to
/// resume by. It does record one, so this reads it back the first time the
/// agent stops talking (the earliest moment the session file exists) and writes
/// it where restore already looks.
///
/// Matched by working directory, not by "the newest session": under one Codex
/// account with several terminals open, the newest belongs to whichever agent
/// last spoke.
fn learn_assigned_session_id<R: Runtime>(app: &AppHandle<R>, terminal_id: &str, info: &TerminalInfo) {
    if info.session_uuid.is_some() {
        return;
    }
    let Some(account_id) = info.account_id.as_deref() else { return };
    let Some(db) = app.try_state::<DbConnection>() else { return };

    let row: Option<(String, String)> = {
        let Ok(conn) = db.0.lock() else { return };
        conn.query_row("SELECT provider, config_dir_path FROM accounts WHERE id = ?1", rusqlite::params![account_id], |r| Ok((r.get(0)?, r.get(1)?))).ok()
    };
    let Some((provider, config_dir)) = row else { return };
    if !matches!(crate::providers::spec(&provider).map(|s| s.session), Some(crate::providers::SessionControl::AssignedId { .. })) {
        return;
    }

    let Some(session_id) = crate::usage_meter::assigned_session_id(&config_dir, &info.cwd) else { return };
    if let Ok(conn) = db.0.lock() {
        let _ = conn.execute("UPDATE open_terminals SET session_uuid = ?2 WHERE id = ?1", rusqlite::params![terminal_id, session_id]);
    }

    // The live handle too, so closing and reopening within this run resumes as
    // well — not only a relaunch, which is the only path the table covers.
    if let Some(registry) = app.try_state::<TerminalRegistry>() {
        if let Ok(mut map) = registry.0.lock() {
            if let Some(handle) = map.get_mut(terminal_id) {
                handle.info.session_uuid = Some(session_id.clone());
            }
        }
    }
    log::info!("learned {provider} session {session_id} for terminal {terminal_id}");
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
        match conn.query_row("SELECT config_dir_path FROM accounts WHERE id = ?1", rusqlite::params![account_id], |row| row.get(0)) {
            Ok(dir) => Some(dir),
            Err(e) => {
                // Loud, because the previous version wrote `config_dir` — a
                // column that doesn't exist — and `.ok()` turned that into
                // "this account has no config dir". Usage recording looked
                // implemented and recorded nothing for weeks.
                log::warn!("usage: no config dir for account {account_id}: {e}");
                None
            }
        }
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
    /// The user's own instructions for a board task, typed when picking it up.
    /// Added to the brief rather than replacing it, which is what tells them
    /// apart from `prompt`. Defaulted so a client that predates the field —
    /// the mobile bridge — keeps working.
    #[serde(default)]
    pub notes: Option<String>,
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

        let worktree = crate::commands::sessions::create_worktree_on_branch(app.clone(), project_id.clone(), branch, task.account_id.clone()).await;
        let (session, reused) = match worktree {
            Ok(worktree) => (worktree.session, worktree.reused),
            Err(e) => {
                failures.push(format!("{}: {e}", task.branch));
                continue;
            }
        };
        let target = TerminalTarget { kind: TargetKind::Session, id: session.id.clone() };
        let notes = task.notes.as_deref().map(str::trim).filter(|n| !n.is_empty());

        // Reopening a task whose worktree was already there goes back to it —
        // to its terminal if that's still open, to its conversation if not.
        if reused {
            if let Some(info) = terminal_for_session(&app, &session.id) {
                opened.push(info);
                continue;
            }
            // Only under the same account: the transcript lives in that
            // account's config dir, and another account can't resume it.
            let conversation = (session.account_id == task.account_id)
                .then(|| crate::commands::sessions::list_resumable_conversations_blocking(&app, &session.id).ok())
                .flatten()
                .and_then(|list| list.into_iter().next());
            if let Some(conversation) = conversation {
                // No prompt unless there are new notes: the brief was that
                // conversation's first message already, and sending it again
                // would restart the task.
                let notes_prompt = match (notes, task.task.as_ref(), session.worktree_path.as_deref()) {
                    (Some(notes), Some(brief), Some(worktree)) => Some(crate::task_sources::add_notes_to_brief(std::path::Path::new(worktree), brief, notes)),
                    _ => None,
                };
                match open_terminal_blocking(
                    app.clone(),
                    target.clone(),
                    Some(task.account_id.clone()),
                    Some(InitCommand::Resume),
                    Some(conversation.provider_session_id),
                    notes_prompt,
                    cols,
                    rows,
                ) {
                    Ok(info) => {
                        opened.push(info);
                        continue;
                    }
                    // Falls through to a fresh start: the work is in the files
                    // either way, and a terminal that opens beats one that doesn't.
                    Err(e) => log::warn!("open_task_worktrees: could not resume {}: {e}", task.branch),
                }
            }
        }

        // An explicit prompt wins: a row the user typed over is a row they
        // meant. Otherwise the brief goes to disk and the prompt points at it.
        let prompt = match (task.prompt.filter(|p| !p.trim().is_empty()), task.task.as_ref(), session.worktree_path.as_deref()) {
            (Some(typed), _, _) => Some(typed),
            (None, Some(brief), Some(worktree)) => Some(crate::task_sources::write_task_brief(std::path::Path::new(worktree), brief, notes)),
            _ => None,
        };
        match open_terminal_blocking(app.clone(), target, Some(task.account_id), Some(InitCommand::Start), None, prompt, cols, rows) {
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
    // Nothing spent since the last reading means the reading is still true. A
    // quota moves when turns are spent and at no other time, so two idle
    // terminals must not cost a Claude Code launch every five minutes.
    if !crate::usage_scraper::spend_since_last_scrape(&db, account_id) {
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

    /// A pane narrower than this killed opencode outright: its TUI segfaults
    /// inside Bun at 28 columns or fewer, and the row count is irrelevant
    /// (20×20 dies, 24×40 does not). From the outside it read as "opencode
    /// doesn't open" — the terminal appeared, the process died, the shell prompt
    /// came back.
    ///
    /// The floor is what the PTY is told, not what the pane is: a six-up grid
    /// legitimately produces panes this narrow, and the alternative to wrapping
    /// text wider than the pane shows is a CLI that crashes.
    #[test]
    fn a_narrow_pane_never_reaches_a_cli_as_a_lethal_width() {
        assert!(MIN_PTY_COLS >= 30, "measured: 28 columns crashes opencode, 30 does not");

        // The clamp both sites apply.
        for pane in [0u16, 1, 12, 20, 28] {
            assert_eq!(pane.max(MIN_PTY_COLS), MIN_PTY_COLS, "a {pane}-column pane must be widened before it reaches the CLI");
        }
        // And a real width is passed through untouched.
        for pane in [MIN_PTY_COLS, 80, 200] {
            assert_eq!(pane.max(MIN_PTY_COLS), pane, "a usable width must not be rewritten");
        }
    }
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
            endpoint: None,
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

    /// Codex can't be told which session to start, but it can be told which to
    /// resume — once Hangar has read back the id it chose. Without this a
    /// relaunch reopened the panel on an empty conversation, which looked like
    /// continuity right up until you scrolled.
    #[test]
    fn a_codex_terminal_resumes_by_the_id_it_assigned_itself() {
        if !crate::providers::is_provider_available("codex") {
            return; // same reasoning as the test below: the shape needs the CLI
        }
        let codex = AccountForTerminal {
            provider: "codex".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/codex-home".to_string(),
            name: "Codex".to_string(),
            endpoint: None,
        };
        let line = account_command_line(&codex, InitCommand::Resume, Some("019fa0ee-92fa-7da0"), None).unwrap();
        // Unquoted because a uuid needs no quoting — `shell_quote` only adds
        // them where the shell would otherwise split the word.
        assert!(line.contains("resume 019fa0ee-92fa-7da0"), "got: {line}");
        assert!(line.starts_with("CODEX_HOME="), "and still isolated to this account: {line}");
    }

    /// Before the id is known there is nothing to resume by, and resuming "the
    /// most recent session" would be a guess dressed as continuity.
    #[test]
    fn a_codex_terminal_with_no_learned_id_starts_fresh() {
        if !crate::providers::is_provider_available("codex") {
            return;
        }
        let codex = AccountForTerminal {
            provider: "codex".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cx".to_string(),
            name: "Codex".to_string(),
            endpoint: None,
        };
        let line = account_command_line(&codex, InitCommand::Resume, None, None).unwrap();
        assert!(!line.contains("resume"), "got: {line}");
    }

    /// El ajuste de la interfaz tiene que llegar al CLI. Las terminales nunca
    /// habían llevado modo — `Mode` existía y solo lo usaban los turnos
    /// headless — así que el desplegable prometía algo que el proceso ignoraba.
    #[test]
    fn el_modo_global_llega_a_la_linea_de_comandos() {
        let claude = account("sub", "/Users/killua/.claude-David");
        let Ok(line) = account_command_line_with_mode(&claude, InitCommand::Start, None, None, Some(crate::providers::Mode::Yolo)) else {
            return; // sin el CLI instalado no hay forma que comprobar
        };
        assert!(line.contains("--permission-mode bypassPermissions"), "got: {line}");
    }

    /// Y sin modo, la línea es exactamente la de antes: el puente no puede
    /// cambiar cómo se abre una terminal por el hecho de existir.
    #[test]
    fn sin_modo_la_linea_no_cambia() {
        let claude = account("sub", "/Users/killua/.claude-David");
        let Ok(with_none) = account_command_line_with_mode(&claude, InitCommand::Start, None, None, None) else { return };
        let plain = account_command_line(&claude, InitCommand::Start, None, None).unwrap();
        assert_eq!(with_none, plain);
        assert!(!plain.contains("--permission-mode"), "got: {plain}");
    }

    /// Un login no ejecuta nada del proyecto, y `claude auth login
    /// --permission-mode …` no es un comando válido.
    #[test]
    fn un_login_no_lleva_modo() {
        let claude = account("sub", "/Users/killua/.claude-David");
        let Ok(line) = account_command_line_with_mode(&claude, InitCommand::Login, None, None, Some(crate::providers::Mode::Yolo)) else { return };
        assert!(!line.contains("--permission-mode"), "got: {line}");
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

    /// On Unix a backslash is a legal character in a filename, so `shell_path`
    /// must leave it alone — rewriting it would point at a different file.
    #[cfg(unix)]
    #[test]
    fn a_unix_path_keeps_every_character_it_came_with() {
        assert_eq!(shell_path("/Users/me/proj"), "/Users/me/proj");
        assert_eq!(shell_path(r"/Users/me/od\d name"), r"'/Users/me/od\d name'");
    }

    /// The Windows half of the same seam: `git worktree` hands back
    /// `C:\Users\me\proj`, and the script that consumes it runs in Git's bash,
    /// where the backslashes would be read as escapes. Forward slashes are
    /// understood by both, and they leave the path unquoted — which is what
    /// keeps the command readable in the account row.
    #[cfg(windows)]
    #[test]
    fn a_windows_path_is_handed_to_bash_with_separators_it_understands() {
        assert_eq!(shell_path(r"C:\Users\me\proj"), "C:/Users/me/proj");
        assert_eq!(shell_path(r"C:\Users\me\My Projects\app"), "'C:/Users/me/My Projects/app'");
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
            endpoint: None,
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
            endpoint: None,
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

    /// The user's language preference has to reach the CLI, and reach it
    /// *only* when there is one.
    ///
    /// No preferences must mean **no flag at all** — not a flag pointing at an
    /// empty file. The command line is what the picker previews, so a flag
    /// that says nothing costs a line of something the user reads to decide
    /// what is about to run.
    #[test]
    fn las_preferencias_llegan_al_cli_y_solo_cuando_existen() {
        let claude = AccountForTerminal {
            provider: "claude".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cl".to_string(),
            name: "Claude".to_string(),
            endpoint: None,
        };
        if !crate::providers::is_provider_available("claude") {
            return;
        }

        let without = account_command_line_full(&claude, InitCommand::Start, None, None, None, None, None, None, &[], &[], None).unwrap();
        assert!(!without.contains("--append-system-prompt-file"), "sin preferencias no se pasa nada: {without}");

        let prefs = sample_instructions();
        let with = account_command_line_full(&claude, InitCommand::Start, None, None, None, Some(&prefs), None, None, &[], &[], None).unwrap();
        assert!(with.contains("--append-system-prompt-file"), "got: {with}");
        assert!(with.contains("/tmp/hangar/prefs.md"), "got: {with}");

        // Un login no ejecuta nada del proyecto: no hay conversación a la que
        // ponerle idioma, y `claude auth login --append-system-prompt-file` no
        // es un comando válido.
        let login = account_command_line_full(&claude, InitCommand::Login, None, None, None, Some(&prefs), None, None, &[], &[], None).unwrap();
        assert!(!login.contains("--append-system-prompt-file"), "un login no lleva instrucciones: {login}");
    }

    fn sample_instructions() -> crate::commands::preferences::ResolvedInstructions {
        crate::commands::preferences::ResolvedInstructions {
            file: std::path::PathBuf::from("/tmp/hangar/prefs.md"),
            inline: "Responde siempre en español.".to_string(),
        }
    }

    /// Codex no acepta la bandera de Claude — pasársela abriría la terminal con
    /// `error: unexpected argument`, que es el fallo con el que empezó el
    /// soporte de Codex. Sí acepta la clave de configuración equivalente.
    #[test]
    fn codex_recibe_las_instrucciones_por_configuracion_y_nunca_por_la_bandera_de_claude() {
        let codex = AccountForTerminal {
            provider: "codex".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cx".to_string(),
            name: "Codex".to_string(),
            endpoint: None,
        };
        if !crate::providers::is_provider_available("codex") {
            return;
        }

        let prefs = sample_instructions();
        let line = account_command_line_full(&codex, InitCommand::Start, None, None, None, Some(&prefs), None, None, &[], &[], None).unwrap();
        assert!(!line.contains("--append-system-prompt-file"), "codex la rechaza: {line}");
        assert!(!line.contains("prefs.md"), "ni la ruta suelta: {line}");
        assert!(line.contains("developer_instructions="), "pero sí la clave que entiende: {line}");
    }

    /// Archify trae en su propio SKILL.md la orden de ejecutar
    /// `scripts/check-update.mjs`, que sale a la red. El README de Hangar
    /// promete que nada sale de la máquina, y una skill instalada *desde*
    /// Hangar es responsabilidad de Hangar.
    ///
    /// Esta mitad comprueba la derivación: de los symlinks que hay de verdad al
    /// entorno. La otra —que ese entorno se pinta en la línea— está en
    /// `el_entorno_de_una_skill_se_pinta_delante_del_binario`.
    #[test]
    fn una_skill_que_llama_a_casa_sale_callada() {
        let base = std::env::temp_dir().join(format!("hangar-silencing-{}", uuid::Uuid::new_v4()));
        let skills = base.join(".claude").join("skills");
        std::fs::create_dir_all(skills.join("otra")).unwrap();

        // Una skill cualquiera no gana variables: la tabla es corta a propósito.
        let linked = crate::commands::skills::linked_skills(&base);
        assert!(crate::commands::skills::silencing_env(&linked).is_empty(), "no se calla lo que no llama: {linked:?}");

        std::fs::create_dir_all(skills.join("archify")).unwrap();
        let linked = crate::commands::skills::linked_skills(&base);
        let env = crate::commands::skills::silencing_env(&linked);
        assert_eq!(env, vec![("ARCHIFY_UPDATE_CHECK_DISABLED".to_string(), "1".to_string())], "got: {env:?}");

        // Y el directorio de diagramas, que solo tiene sentido con la skill.
        let app_data = base.join("app-data");
        let diagrams = crate::commands::skills::diagrams_env(&linked, &app_data, "p1");
        assert_eq!(diagrams.len(), 1, "got: {diagrams:?}");
        assert_eq!(diagrams[0].0, "HANGAR_DIAGRAMS");
        assert!(std::path::Path::new(&diagrams[0].1).is_dir(), "el directorio se crea, no se promete: {diagrams:?}");
        assert!(
            crate::commands::skills::diagrams_env(&["otra".to_string()], &app_data, "p1").is_empty(),
            "sin la skill, una variable que nombra un directorio que nadie va a usar es ruido"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// El entorno de una skill se enseña, no se pone a escondidas: la línea que
    /// previsualiza el selector tiene que ser la que corre.
    #[test]
    fn el_entorno_de_una_skill_se_pinta_delante_del_binario() {
        if !crate::providers::is_provider_available("claude") {
            return;
        }
        let claude = AccountForTerminal {
            provider: "claude".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cl".to_string(),
            name: "Claude".to_string(),
            endpoint: None,
        };
        let env = vec![
            ("ARCHIFY_UPDATE_CHECK_DISABLED".to_string(), "1".to_string()),
            ("HANGAR_DIAGRAMS".to_string(), "/tmp/app-data/diagrams/p1".to_string()),
        ];

        let sin = account_command_line_full(&claude, InitCommand::Start, None, None, None, None, None, None, &[], &[], None).unwrap();
        assert!(!sin.contains("ARCHIFY_UPDATE_CHECK_DISABLED"), "sin entorno no se inventa nada: {sin}");

        let con = account_command_line_full(&claude, InitCommand::Start, None, None, None, None, None, None, &[], &env, None).unwrap();
        assert!(con.contains("ARCHIFY_UPDATE_CHECK_DISABLED=1"), "got: {con}");
        assert!(con.contains("HANGAR_DIAGRAMS=/tmp/app-data/diagrams/p1"), "got: {con}");
        let first_env = con.find("ARCHIFY_UPDATE_CHECK_DISABLED").unwrap();
        assert!(first_env < con.find("claude").unwrap(), "las variables van antes del binario: {con}");

        // Un login no ejecuta nada del proyecto, así que no hay skill en juego.
        let login = account_command_line_full(&claude, InitCommand::Login, None, None, None, None, None, None, &[], &env, None).unwrap();
        assert!(!login.contains("ARCHIFY_UPDATE_CHECK_DISABLED"), "un login no lleva skills: {login}");
        assert!(!login.contains("HANGAR_DIAGRAMS"), "ni dónde dejar diagramas: {login}");
    }

    /// El directorio de skills entra solo cuando el proyecto tiene alguna
    /// marcada, y solo para los CLI donde está verificado que funciona.
    #[test]
    fn el_directorio_de_skills_llega_solo_a_quien_lo_entiende() {
        let dir = std::path::Path::new("/tmp/hangar/projects/p1");

        let claude = AccountForTerminal {
            provider: "claude".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cl".to_string(),
            name: "Claude".to_string(),
            endpoint: None,
        };
        if crate::providers::is_provider_available("claude") {
            let without = account_command_line_full(&claude, InitCommand::Start, None, None, None, None, None, None, &[], &[], None).unwrap();
            assert!(!without.contains("--add-dir"), "sin skills marcadas no se pasa nada: {without}");

            let with = account_command_line_full(&claude, InitCommand::Start, None, None, None, None, Some(dir), None, &[], &[], None).unwrap();
            assert!(with.contains("--add-dir"), "got: {with}");
            assert!(with.contains("/tmp/hangar/projects/p1"), "got: {with}");

            // Un login no trabaja en el proyecto, así que no necesita sus skills.
            let login = account_command_line_full(&claude, InitCommand::Login, None, None, None, None, Some(dir), None, &[], &[], None).unwrap();
            assert!(!login.contains("--add-dir"), "un login no lleva skills: {login}");
        }

        let codex = AccountForTerminal {
            provider: "codex".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cx".to_string(),
            name: "Codex".to_string(),
            endpoint: None,
        };
        if crate::providers::is_provider_available("codex") {
            let line = account_command_line_full(&codex, InitCommand::Start, None, None, None, None, Some(dir), None, &[], &[], None).unwrap();
            assert!(!line.contains("--add-dir"), "codex no usa esa bandera: {line}");
        }
    }

    /// El grafo entra por bandera y **nunca** con `--strict-mcp-config`
    /// (`GRAPHIFY_TRD.md` §5.2): esa bandera apagaría los servidores MCP del
    /// usuario a cambio de añadir el nuestro. Aditivo o nada.
    #[test]
    fn el_grafo_entra_por_bandera_y_es_aditivo() {
        let config = std::path::Path::new("/tmp/hangar/graphs/project-p1/mcp-config.json");

        let claude = AccountForTerminal {
            provider: "claude".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cl".to_string(),
            name: "Claude".to_string(),
            endpoint: None,
        };
        if crate::providers::is_provider_available("claude") {
            // El caso normal: sin grafo construido no se añade nada, y la
            // terminal abre exactamente como abría antes de que esto existiera.
            let without = account_command_line_full(&claude, InitCommand::Start, None, None, None, None, None, None, &[], &[], None).unwrap();
            assert!(!without.contains("--mcp-config"), "sin grafo no se pasa bandera: {without}");

            let with = account_command_line_full(&claude, InitCommand::Start, None, None, None, None, None, Some(config), &[], &[], None).unwrap();
            assert!(with.contains("--mcp-config"), "got: {with}");
            assert!(with.contains("mcp-config.json"), "got: {with}");
            assert!(
                !with.contains("--strict-mcp-config"),
                "esa bandera ignora toda otra configuración MCP: le apagaría al usuario sus propios servidores. Got: {with}"
            );

            // Un login no consulta código.
            let login = account_command_line_full(&claude, InitCommand::Login, None, None, None, None, None, Some(config), &[], &[], None).unwrap();
            assert!(!login.contains("--mcp-config"), "un login no lleva grafo: {login}");
        }

        let codex = AccountForTerminal {
            provider: "codex".to_string(),
            billing: "sub".to_string(),
            config_dir_path: "/tmp/cx".to_string(),
            name: "Codex".to_string(),
            endpoint: None,
        };
        if crate::providers::is_provider_available("codex") {
            let line = account_command_line_full(&codex, InitCommand::Start, None, None, None, None, None, Some(config), &[], &[], None).unwrap();
            assert!(!line.contains("--mcp-config"), "codex no usa esa bandera: {line}");
        }
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
            endpoint: None,
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

    /// A custom account is the Kimi case with the address read off the account
    /// instead of a constant. It has to be on the command line — that string is
    /// what the picker shows, and the address is where the code is about to go —
    /// and the key must not be.
    #[test]
    fn a_custom_account_runs_claude_against_its_own_endpoint() {
        let custom = AccountForTerminal {
            provider: "custom".to_string(),
            billing: "key".to_string(),
            config_dir_path: "/tmp/ds".to_string(),
            name: "DeepSeek".to_string(),
            endpoint: Some(AccountEndpoint { base_url: "https://api.deepseek.com/anthropic".to_string(), model: Some("deepseek-chat".to_string()) }),
        };
        let result = account_command_line(&custom, InitCommand::Start, None, None);
        if crate::providers::is_provider_available("custom") {
            let line = result.expect("custom runs through claude, which is installed here");
            assert!(line.contains("ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic "), "got: {line}");
            assert!(line.contains("ANTHROPIC_MODEL=deepseek-chat "), "got: {line}");
            assert!(line.contains(" claude"), "the binary is claude: {line}");
            assert!(!line.contains("ANTHROPIC_AUTH_TOKEN"), "the key travels as environment, never on the command line: {line}");
        } else {
            assert!(result.is_err());
        }
    }

    /// No model column means no `ANTHROPIC_MODEL` — the endpoint is then asked
    /// for whatever Claude Code would ask Anthropic for, which is what a gateway
    /// that maps Claude's own names expects.
    #[test]
    fn an_endpoint_without_a_model_sets_only_the_address() {
        let endpoint = AccountEndpoint::from_columns(Some("http://localhost:3001".to_string()), Some("  ".to_string())).unwrap();
        assert_eq!(endpoint.env(), vec![("ANTHROPIC_BASE_URL", "http://localhost:3001")]);
        assert_eq!(AccountEndpoint::from_columns(None, Some("m".to_string())), None, "a model with no address is not an endpoint");
        assert_eq!(AccountEndpoint::from_columns(Some(String::new()), None), None);
    }

    /// The hazard `IDENTITY_ENV_VARS` exists for, in its worst form: an endpoint
    /// exported in the shell that launched Hangar would silently send a
    /// subscription account's terminal somewhere else.
    #[test]
    fn an_inherited_endpoint_never_reaches_an_account_terminal() {
        let base = vec![
            ("ANTHROPIC_BASE_URL".to_string(), "https://inherited.example".to_string()),
            ("ANTHROPIC_AUTH_TOKEN".to_string(), "inherited-token".to_string()),
            ("PATH".to_string(), "/usr/bin".to_string()),
        ];
        let scoped = merge_env(base.clone(), &[], true);
        assert_eq!(value_of(&scoped, "ANTHROPIC_BASE_URL"), None);
        assert_eq!(value_of(&scoped, "ANTHROPIC_AUTH_TOKEN"), None);
        assert_eq!(value_of(&scoped, "PATH"), Some("/usr/bin"));

        let own = vec![("ANTHROPIC_BASE_URL".to_string(), "https://api.deepseek.com/anthropic".to_string())];
        let custom = merge_env(base.clone(), &own, true);
        assert_eq!(value_of(&custom, "ANTHROPIC_BASE_URL"), Some("https://api.deepseek.com/anthropic"), "the account's own endpoint replaces the inherited one");

        let plain = merge_env(base, &[], false);
        assert_eq!(value_of(&plain, "ANTHROPIC_BASE_URL"), Some("https://inherited.example"), "a terminal with no account is the user's own shell");
    }

    /// Which account switches keep the conversation. Onto a fallback, yes; back
    /// from one, no — see `carries_conversation` for why that is asymmetric.
    #[test]
    fn the_conversation_follows_onto_a_fallback_but_not_back() {
        assert!(carries_conversation("claude", "claude"));
        assert!(carries_conversation("claude", "custom"));
        assert!(carries_conversation("custom", "custom"));
        assert!(!carries_conversation("custom", "claude"));
        assert!(!carries_conversation("claude", "codex"));
        assert!(!carries_conversation("codex", "custom"));
    }

    /// An API-key provider has no interactive login, and offering one would
    /// open a terminal that runs a command that doesn't exist.
    ///
    /// Needs a real `claude` on the machine — Kimi is Claude Code pointed at
    /// Moonshot, so `account_command_line` checks the binary is installed
    /// before it ever reaches the refusal this asserts on. Without it the
    /// error is the truthful "the `claude` CLI isn't installed" instead, which
    /// is a different correct answer to a different question.
    #[test]
    #[ignore = "needs the real `claude` CLI installed — see doc comment"]
    fn a_key_only_provider_refuses_a_login_command() {
        let kimi = AccountForTerminal {
            provider: "kimi".to_string(),
            billing: "key".to_string(),
            config_dir_path: "/tmp/kimi".to_string(),
            name: "Kimi".to_string(),
            endpoint: None,
        };
        let err = account_command_line(&kimi, InitCommand::Login, None, None).unwrap_err();
        assert!(err.contains("API key"), "the message has to explain why there's no login: {err}");
    }

    #[test]
    fn a_command_without_an_account_is_refused() {
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(seed())));
        app.manage(TerminalRegistry::new());

        let result = open_terminal_blocking(
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
        let result = open_terminal_blocking(app.handle().clone(), TerminalTarget { kind: TargetKind::Project, id: "proj1".into() }, None, None, None, None, 80, 24);
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

        let info = open_terminal_blocking(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp".into() }, None, None, None, None, 80, 24)
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

    /// El orden de las pulsaciones tiene que salir del canal, no de una carrera.
    ///
    /// `TerminalPane.tsx` invoca `write_terminal` **sin esperar** la respuesta,
    /// así que lo único que ordena las teclas es que la IPC despache en serie y
    /// que encolar sea instantáneo. Si algún día alguien hace este comando
    /// asíncrono «para que no bloquee», este test es el que lo caza: dos
    /// escrituras repartidas a hilos distintos competirían y `ls` podría llegar
    /// como `sl`.
    #[test]
    fn what_you_type_reaches_the_shell_in_the_order_you_typed_it() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("hangar-test-order-{}", Uuid::new_v4()));
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

        let info = open_terminal_blocking(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp".into() }, None, None, None, None, 80, 24)
            .expect("opening a terminal in a real directory must succeed");

        // Una orden partida en trozos: solo llega entera si se conserva el orden.
        for piece in ["echo hangar", "-order", "-ok\n"] {
            write_terminal(handle.clone(), info.id.clone(), piece.to_string()).unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(20);
        let mut seen = false;
        while Instant::now() < deadline {
            let registry = handle.state::<TerminalRegistry>();
            let map = registry.0.lock().unwrap();
            let buffer = map.get(&info.id).unwrap().scrollback.lock().unwrap();
            let text = String::from_utf8_lossy(&buffer.iter().copied().collect::<Vec<_>>()).to_string();
            drop(buffer);
            drop(map);
            // Dos veces: el eco de lo tecleado y la salida del propio shell.
            if text.matches("hangar-order-ok").count() >= 2 {
                seen = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(seen, "los trozos tienen que llegar en orden y formar la orden entera");

        close_terminal(handle.clone(), info.id.clone()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Escribir no puede quedarse con el candado global del registro: era la
    /// forma exacta que congelaba la app entera, porque los tres ticks (salida
    /// a 60 Hz, estados a 1 s, cola móvil a 10 Hz) lo necesitan.
    #[test]
    fn writing_does_not_sit_on_the_global_registry_lock() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("hangar-test-lock-{}", Uuid::new_v4()));
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

        let info = open_terminal_blocking(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp".into() }, None, None, None, None, 80, 24)
            .expect("opening a terminal in a real directory must succeed");

        // Mucho más de lo que un humano teclea, para que si algo se bloqueara
        // por escritura se notara en el reloj.
        let started = Instant::now();
        for _ in 0..500 {
            write_terminal(handle.clone(), info.id.clone(), "x".to_string()).unwrap();
        }
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_secs(2), "500 escrituras encoladas tardaron {elapsed:?}: algo está bloqueando");

        // Y el candado sigue tomándose sin espera: si `write_terminal` lo
        // retuviera, esto competiría con él.
        assert!(handle.state::<TerminalRegistry>().0.lock().is_ok());

        close_terminal(handle.clone(), info.id.clone()).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Una terminal que ya no existe se dice, no se traga.
    #[test]
    fn writing_to_a_terminal_that_is_gone_is_an_error() {
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(seed())));
        app.manage(TerminalRegistry::new());
        let err = write_terminal(app.handle().clone(), "no-existe".to_string(), "hola".to_string()).unwrap_err();
        assert!(err.contains("no terminal with id"), "got: {err}");
    }

    /// El escenario del cuelgue, montado a propósito y con reloj.
    ///
    /// Una flota de seis terminales; en una de ellas corre un `sleep`, que **no
    /// lee su entrada**, así que el búfer de la tty se llena y la escritura al
    /// PTY se para de verdad. Contra eso se mide lo que antes se moría con ella:
    ///
    /// - que escribir a la atascada vuelva al instante (o falle diciendo que la
    ///   cola está llena), pero nunca se quede colgado;
    /// - que el candado global siga disponible, que es lo que necesitan los tres
    ///   ticks (salida a 60 Hz, estados a 1 s, cola móvil a 10 Hz);
    /// - que las otras cinco sigan aceptando y sacando texto.
    ///
    /// Con el código anterior este test se bloquea en la primera escritura que
    /// llena el búfer: era un comando síncrono que escribía al PTY con el
    /// candado global puesto. Comprobado volviendo atrás el cambio.
    #[test]
    fn a_terminal_that_stops_reading_does_not_take_the_others_with_it() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("hangar-stress-{}", Uuid::new_v4()));
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

        let open = || {
            open_terminal_blocking(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp".into() }, None, None, None, None, 80, 24)
                .expect("opening a terminal in a real directory must succeed")
        };
        let stuck = open();
        let others: Vec<TerminalInfo> = (0..5).map(|_| open()).collect();

        // Que deje de leer: `sleep` no toca stdin, así que todo lo que llegue
        // se queda en el búfer de la tty hasta que se llene.
        write_terminal(handle.clone(), stuck.id.clone(), "sleep 30\n".to_string()).unwrap();
        std::thread::sleep(Duration::from_millis(800));

        // Ocho megas contra una terminal que no lee. Sin el arreglo, una de
        // estas llamadas no vuelve.
        let started = Instant::now();
        let chunk = "x".repeat(8 * 1024);
        let mut refused = 0;
        for _ in 0..20_000 {
            match write_terminal(handle.clone(), stuck.id.clone(), chunk.clone()) {
                Ok(()) => {}
                Err(e) => {
                    assert!(e.contains("not reading its input"), "el rechazo tiene que decir por qué: {e}");
                    refused += 1;
                }
            }
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "escribir a una terminal atascada bloqueó el hilo: {:?}",
                started.elapsed()
            );
        }
        let hammering = started.elapsed();
        // El test comprueba su propia premisa: si nada se rechazó, el hilo
        // escritor iba drenando y el PTY nunca llegó a atascarse — entonces
        // esto no estaría midiendo el escenario del cuelgue, y pasar no
        // significaría nada.
        assert!(refused > 0, "la cola nunca se llenó: el PTY no llegó a bloquearse y el test no prueba lo que dice");

        // El candado global tiene que seguir tomándose sin espera: es el que
        // comparten los tres ticks, y perderlo era lo que enmudecía la app.
        let lock_at = Instant::now();
        {
            let registry = handle.state::<TerminalRegistry>();
            let map = registry.0.lock().expect("el candado global tiene que estar libre");
            assert_eq!(map.len(), 6, "las seis siguen registradas");
        }
        assert!(lock_at.elapsed() < Duration::from_secs(1), "el candado global tardó {:?}", lock_at.elapsed());

        // Y los ticks, que son los que de verdad se congelaban.
        tick_terminal_output(&handle);
        tick_terminal_states(&handle);
        tick_mobile_tail(&handle);

        // Las otras cinco tienen que seguir vivas: aceptar entrada y sacar texto.
        for (i, info) in others.iter().enumerate() {
            write_terminal(handle.clone(), info.id.clone(), format!("echo hangar-alive-{i}\n")).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut alive = 0;
        while Instant::now() < deadline && alive < others.len() {
            alive = others
                .iter()
                .enumerate()
                .filter(|(i, info)| {
                    let registry = handle.state::<TerminalRegistry>();
                    let map = registry.0.lock().unwrap();
                    let buffer = map.get(&info.id).unwrap().scrollback.lock().unwrap();
                    let text = String::from_utf8_lossy(&buffer.iter().copied().collect::<Vec<_>>()).to_string();
                    text.matches(&format!("hangar-alive-{i}")).count() >= 2
                })
                .count();
            std::thread::sleep(Duration::from_millis(100));
        }
        assert_eq!(alive, others.len(), "solo {alive} de {} terminales sanas respondieron", others.len());

        eprintln!("  [estrés] 20000 escrituras de 8 KB en {hammering:?}, {refused} rechazadas por cola llena");

        close_terminal(handle.clone(), stuck.id.clone()).unwrap();
        for info in &others {
            close_terminal(handle.clone(), info.id.clone()).unwrap();
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Attaching hands over the scrollback *and* the pane must not then be sent
    /// the same bytes again by the next flush.
    ///
    /// This is the one thing the batching could plausibly get wrong: output is
    /// queued on the sink now instead of being sent per read, so a sink
    /// installed with anything already pending would print the tail of the
    /// snapshot twice. `attach_terminal` starts it empty under the same lock
    /// the reader appends through, and this pins that.
    #[test]
    fn attaching_replays_the_scrollback_without_the_pane_seeing_it_twice() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("hangar-test-batch-{}", Uuid::new_v4()));
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

        let info = open_terminal_blocking(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp".into() }, None, None, None, None, 80, 24).unwrap();

        // Printed *before* anyone is watching, so it can only reach a pane
        // through the scrollback `attach_terminal` returns.
        write_terminal(handle.clone(), info.id.clone(), "echo hangar-before-attach\n".to_string()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(20);
        let mut in_scrollback = false;
        while Instant::now() < deadline {
            let registry = handle.state::<TerminalRegistry>();
            let map = registry.0.lock().unwrap();
            let buffer = map.get(&info.id).unwrap().scrollback.lock().unwrap();
            let text = String::from_utf8_lossy(&buffer.iter().copied().collect::<Vec<_>>()).to_string();
            drop(buffer);
            drop(map);
            // Twice: the shell echoes the command line, then runs it.
            if text.matches("hangar-before-attach").count() >= 2 {
                in_scrollback = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(in_scrollback, "the marker must be in the scrollback before attaching");

        let received = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink_log = received.clone();
        let channel = Channel::new(move |body| {
            if let tauri::ipc::InvokeResponseBody::Json(json) = body {
                let value: serde_json::Value = serde_json::from_str(&json).unwrap();
                let encoded = value["base64"].as_str().unwrap_or_default();
                let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap_or_default();
                sink_log.lock().unwrap().push(String::from_utf8_lossy(&bytes).to_string());
            }
            Ok(())
        });

        let snapshot = attach_terminal(handle.clone(), info.id.clone(), channel).unwrap();
        let replayed = String::from_utf8_lossy(&base64::engine::general_purpose::STANDARD.decode(snapshot).unwrap()).to_string();
        assert!(replayed.contains("hangar-before-attach"), "attach must replay what the terminal printed while nobody watched");

        // Whatever the flusher sends from here on is new output only.
        for _ in 0..5 {
            std::thread::sleep(OUTPUT_FLUSH_INTERVAL);
            tick_terminal_output(&handle);
        }
        let after_attach = received.lock().unwrap().concat();
        assert!(
            !after_attach.contains("hangar-before-attach"),
            "the pane must not be sent bytes the scrollback snapshot already carried, got: {after_attach:?}"
        );

        // And the batched path does deliver what comes next.
        write_terminal(handle.clone(), info.id.clone(), "echo hangar-after-attach\n".to_string()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut delivered = false;
        while Instant::now() < deadline {
            tick_terminal_output(&handle);
            if received.lock().unwrap().concat().matches("hangar-after-attach").count() >= 2 {
                delivered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(delivered, "output printed while attached must reach the pane through the flush tick");

        close_terminal(handle, info.id).unwrap();
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

        let info = open_terminal_blocking(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp2".into() }, Some("sub1".to_string()), None, None, None, 80, 24)
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

        let old = open_terminal_blocking(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp2".into() }, Some("sub1".to_string()), None, None, None, 80, 24).unwrap();
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

        let old = open_terminal_blocking(handle.clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp2".into() }, Some("sub1".to_string()), None, None, None, 80, 24).unwrap();

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
            minimized: false,
            title: "t".to_string(),
            cwd: "/repo".to_string(),
            kind,
            source_id: source_id.to_string(),
            account_name: None,
            account_id: account_id.map(str::to_string),
            session_uuid: session_uuid.map(str::to_string),
            display_name: None,
            auto_continue: false,
        }
    }

    fn persisted_ids(app: &tauri::App<tauri::test::MockRuntime>) -> Vec<String> {
        let db = app.state::<crate::db::DbConnection>();
        let conn = db.0.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id FROM open_terminals ORDER BY created_at").unwrap();
        stmt.query_map([], |r| r.get(0)).unwrap().filter_map(Result::ok).collect()
    }

    /// opencode 2.x dropped `--agent` from its TUI: a terminal typed with it
    /// died on "Unrecognized flag: --agent". Plan mode now rides on
    /// `OPENCODE_CONFIG`, and never on a login. Skipped where opencode isn't
    /// installed, like the other command-line tests.
    #[test]
    fn an_opencode_terminal_in_plan_mode_starts_without_the_agent_flag() {
        let opencode = AccountForTerminal { provider: "opencode".to_string(), ..account("sub", "/Users/killua/.opencode-test") };
        let context = TerminalContext {
            mode_env: crate::providers::permission_env("opencode", crate::providers::Mode::Plan, Some(std::path::Path::new("/data/opencode/plan.json"))),
            ..TerminalContext::default()
        };
        let plan = Some(crate::providers::Mode::Plan);
        let Ok(start) = terminal_script_full("/repo", &opencode, InitCommand::Start, None, None, plan, &context) else { return };
        assert!(!start.contains("--agent"), "not a TUI flag in opencode 2.x: {start}");
        assert!(start.contains("OPENCODE_CONFIG=/data/opencode/plan.json "), "plan mode goes through the config file: {start}");
        assert!(start.trim_end().ends_with("opencode"), "and adds no flag of its own: {start}");

        let ask = terminal_script_full("/repo", &opencode, InitCommand::Start, None, None, Some(crate::providers::Mode::AskEdits), &TerminalContext::default()).unwrap();
        assert!(!ask.contains("OPENCODE_CONFIG"), "only plan mode needs it: {ask}");
    }

    /// `--settings` is Claude Code's flag and a login starts no session to wait
    /// in: it rides on claude Start/Resume only. Skipped where the CLIs aren't
    /// installed, like the other command-line tests.
    #[test]
    fn the_auto_continue_settings_ride_on_claude_sessions_only() {
        let claude = account("sub", "/Users/killua/.claude-David");
        let file = std::path::Path::new("/data/terminals/t1.settings.json");
        let Ok(start) = account_command_line_full(&claude, InitCommand::Start, Some("u1"), None, None, None, None, None, &[], &[], Some(file)) else { return };
        assert!(start.contains(" --settings /data/terminals/t1.settings.json"), "got: {start}");
        let resume = account_command_line_full(&claude, InitCommand::Resume, Some("u1"), None, None, None, None, None, &[], &[], Some(file)).unwrap();
        assert!(resume.contains(" --settings "), "got: {resume}");
        let login = account_command_line_full(&claude, InitCommand::Login, None, None, None, None, None, None, &[], &[], Some(file)).unwrap();
        assert!(!login.contains("--settings"), "a login waits for nothing: {login}");

        let codex = AccountForTerminal { provider: "codex".to_string(), ..account("sub", "/Users/killua/.codex-test") };
        if let Ok(line) = account_command_line_full(&codex, InitCommand::Start, None, None, None, None, None, None, &[], &[], Some(file)) {
            assert!(!line.contains("--settings"), "not Codex's flag: {line}");
        }
    }

    #[test]
    fn the_auto_continue_choice_is_persisted_for_the_restore() {
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(seed())));
        let mut info = info_for("t1", TargetKind::Project, "proj1", Some("acc1"), Some("uuid-1"));
        info.auto_continue = true;
        persist_terminal(app.handle(), &info);
        let db = app.state::<crate::db::DbConnection>();
        let stored: i64 = db.0.lock().unwrap().query_row("SELECT auto_continue FROM open_terminals WHERE id = 't1'", [], |r| r.get(0)).unwrap();
        assert_eq!(stored, 1);
    }

    fn app_with_shell_project() -> (tauri::App<tauri::test::MockRuntime>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("hangar-test-autocontinue-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = seed();
        conn.execute("INSERT INTO projects (id, name, path, created_at) VALUES ('tmp','Temp',?1,'now')", rusqlite::params![dir.to_string_lossy().to_string()]).unwrap();
        let app = tauri::test::mock_app();
        app.manage(crate::db::DbConnection(Mutex::new(conn)));
        app.manage(TerminalRegistry::new());
        (app, dir)
    }

    fn open_shell(app: &tauri::App<tauri::test::MockRuntime>) -> TerminalInfo {
        open_terminal_blocking(app.handle().clone(), TerminalTarget { kind: TargetKind::Project, id: "tmp".into() }, None, None, None, None, 80, 24).unwrap()
    }

    fn with_auto<T>(app: &tauri::App<tauri::test::MockRuntime>, id: &str, f: impl FnOnce(&mut AutoContinue) -> T) -> T {
        let registry = app.state::<TerminalRegistry>();
        let map = registry.0.lock().unwrap();
        let mut auto = map.get(id).unwrap().auto.lock().unwrap();
        f(&mut auto)
    }

    fn scrollback_text(app: &tauri::App<tauri::test::MockRuntime>, id: &str) -> String {
        let registry = app.state::<TerminalRegistry>();
        let map = registry.0.lock().unwrap();
        let buffer = map.get(id).unwrap().scrollback.lock().unwrap();
        String::from_utf8_lossy(&buffer.iter().copied().collect::<Vec<_>>()).to_string()
    }

    fn set_waiting(app: &tauri::App<tauri::test::MockRuntime>, id: &str) {
        let registry = app.state::<TerminalRegistry>();
        let map = registry.0.lock().unwrap();
        map.get(id).unwrap().activity.lock().unwrap().state = TerminalState::Waiting;
    }

    fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    /// Claude Code's give-up line arms a marked terminal and only a marked one.
    /// With no `rate_limits` on record (a plain shell has no account) the time
    /// is the estimated retry, and says so.
    #[test]
    fn the_give_up_message_arms_only_a_marked_terminal() {
        let (app, dir) = app_with_shell_project();
        let marked = open_shell(&app);
        let unmarked = open_shell(&app);
        with_auto(&app, &marked.id, |a| a.enabled = true);

        for info in [&marked, &unmarked] {
            write_terminal(app.handle().clone(), info.id.clone(), "printf 'so this task will not %s\\n' 'resume on its own'\n".to_string()).unwrap();
        }
        assert!(wait_until(|| with_auto(&app, &marked.id, |a| a.resume_at.is_some())), "the marked terminal must arm");
        assert!(with_auto(&app, &marked.id, |a| a.estimated));
        assert!(wait_until(|| scrollback_text(&app, &unmarked.id).contains("resume on its own")));
        assert!(with_auto(&app, &unmarked.id, |a| a.resume_at.is_none()), "an unmarked terminal is never armed");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The tick types the prompt into the armed, due, idle terminal and nowhere
    /// else — not into one that isn't marked, and not into one mid-turn.
    #[test]
    fn the_tick_types_the_continuation_only_where_it_is_due() {
        let (app, dir) = app_with_shell_project();
        let due = open_shell(&app);
        let unmarked = open_shell(&app);
        let busy = open_shell(&app);
        let past = chrono::Utc::now() - chrono::Duration::seconds(1);
        with_auto(&app, &due.id, |a| {
            a.enabled = true;
            a.resume_at = Some(past);
        });
        with_auto(&app, &unmarked.id, |a| a.resume_at = Some(past));
        with_auto(&app, &busy.id, |a| {
            a.enabled = true;
            a.resume_at = Some(past);
        });
        for info in [&due, &unmarked] {
            set_waiting(&app, &info.id);
        }
        {
            let registry = app.state::<TerminalRegistry>();
            let map = registry.0.lock().unwrap();
            let mut act = map.get(&busy.id).unwrap().activity.lock().unwrap();
            act.state = TerminalState::Working;
            act.last_output = Instant::now();
        }

        tick_auto_continue(app.handle());

        assert!(wait_until(|| scrollback_text(&app, &due.id).contains(crate::auto_continue::CONTINUE_PROMPT)), "the due terminal gets the prompt");
        assert!(with_auto(&app, &due.id, |a| a.resume_at.is_none()), "sent once");
        std::thread::sleep(Duration::from_millis(500));
        assert!(!scrollback_text(&app, &unmarked.id).contains(crate::auto_continue::CONTINUE_PROMPT));
        assert!(!scrollback_text(&app, &busy.id).contains(crate::auto_continue::CONTINUE_PROMPT));
        assert!(with_auto(&app, &busy.id, |a| a.resume_at.is_some()), "mid-turn: still armed for later");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The user typing first is the user deciding; a focus report is not.
    #[test]
    fn typing_cancels_an_armed_continuation_but_a_focus_report_does_not() {
        let (app, dir) = app_with_shell_project();
        let info = open_shell(&app);
        let later = chrono::Utc::now() + chrono::Duration::hours(1);
        with_auto(&app, &info.id, |a| {
            a.enabled = true;
            a.resume_at = Some(later);
        });
        write_terminal(app.handle().clone(), info.id.clone(), "\x1b[I".to_string()).unwrap();
        assert!(with_auto(&app, &info.id, |a| a.resume_at.is_some()));
        write_terminal(app.handle().clone(), info.id.clone(), "l".to_string()).unwrap();
        assert!(with_auto(&app, &info.id, |a| a.resume_at.is_none()));

        let _ = std::fs::remove_dir_all(&dir);
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
        let row = PersistedTerminal { target_kind: "project".into(), source_id: "proj1".into(), account_id: None, session_uuid: None, display_name: None, minimized: false, auto_continue: false };
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
        let with = PersistedTerminal { target_kind: "project".into(), source_id: "proj1".into(), account_id: Some("accR".into()), session_uuid: Some(uuid.into()), display_name: None , minimized: false, auto_continue: false };
        assert_eq!(decide_restore_init(handle, &with), (Some(InitCommand::Resume), Some(uuid.to_string())));

        // Same account, a session id with no transcript on disk → fresh start.
        let without = PersistedTerminal { target_kind: "project".into(), source_id: "proj1".into(), account_id: Some("accR".into()), session_uuid: Some("no-such-uuid".into()), display_name: None , minimized: false, auto_continue: false };
        assert_eq!(decide_restore_init(handle, &without), (Some(InitCommand::Start), None));

        let _ = std::fs::remove_dir_all(&cfg);
    }
}
