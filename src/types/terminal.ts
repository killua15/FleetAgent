import type { BoardTask } from './task';
import type { AccountStatus, Scope } from './account';

/** What a terminal was opened against. A session resolves to its worktree
 * when it has one — that's the case the feature exists for — and a project
 * resolves to its own path. See `src-tauri/src/terminals.rs`. */
export type TerminalTargetKind = 'project' | 'session';

export interface TerminalTarget {
  kind: TerminalTargetKind;
  id: string;
}

export interface TerminalInfo {
  id: string;
  /** Folded in the grid. Comes from the backend because terminal ids are minted
   * per run — anything the frontend remembered under the old id is stale the
   * moment `restore_terminals` reopens them. */
  minimized: boolean;
  /** Project or session name, shown in the pane header. */
  title: string;
  cwd: string;
  kind: TerminalTargetKind;
  /** The project/session id this was opened against, so the view can tell
   * "a terminal for this session already exists" without re-resolving paths. */
  sourceId: string;
  /** The account whose environment this shell runs under, if any. A shell
   * carrying credentials says so in its header rather than looking identical
   * to a plain one. */
  accountName: string | null;
  /** The account id behind `accountName` — lets the view tell when this
   * terminal's account has run out of quota (to show the reopen banner). */
  accountId: string | null;
  /** The Claude Code session id Hangar started this terminal on, when it's an
   * account-scoped claude terminal — what makes the conversation resumable or
   * transferable later. `null` for a plain shell / login / non-claude. */
  sessionUuid: string | null;
  /** A name the user typed for this panel, which wins over `title`. Persisted,
   * so it survives a restart — a name that vanished would be worse than none. */
  displayName: string | null;
  /** Continues on its own once its usage limit resets — the user's per-panel
   * choice. Changing it relaunches the CLI, so the panel gets a new terminal. */
  autoContinue: boolean;
}

/** A terminal's auto-continue state, from `list_auto_continue` and the
 * `terminal-auto-continue` event. */
export interface AutoContinueStatus {
  id: string;
  enabled: boolean;
  /** When Hangar will type the continuation itself — only in the cases Claude
   * Code won't (a reset over a day out, or a relaunch mid-wait). */
  resumeAt: string | null;
  /** `resumeAt` is a retry: no reset time was reported to take it from. */
  estimated: boolean;
  /** Changed mid-turn; applied (CLI relaunched) when the turn ends. */
  pendingRelaunch: boolean;
}

/** The account the out-of-quota banner offers to reopen a terminal under. */
export interface SuggestedAccount {
  accountId: string;
  accountName: string;
}

/** One row of the account switcher on a panel's header chip: the accounts this
 * terminal's project may rotate across, from `list_terminal_rotation_accounts`.
 * Includes the current account and any benched ones — the switcher shows the
 * whole configured pool and disables what can't be picked right now, rather
 * than quietly shrinking the list. */
export interface RotatableAccount {
  accountId: string;
  accountName: string;
  scope: Scope;
  status: AccountStatus;
  /** The account this terminal is running under. */
  current: boolean;
  /** What the project's rotation policy would pick. */
  suggested: boolean;
}

/** One branch of a fan-out: the worktree's name and the account that works in
 * it. N branches means N accounts, which is what makes fanning a prompt out to
 * several agents sustainable — a fan-out on one account just burns that one
 * quota N times as fast. */
export interface FanOutBranch {
  name: string;
  accountId: string;
}

/** One task being picked up in parallel with others.
 *
 * The opposite of a fan-out branch: there, N attempts at one job share a base
 * and get namespaced branches, and you keep one. Here each row is a different
 * job on **its own real branch** — the name the PR, the CI and the team already
 * know — and you keep all of them. Hence the per-row base: two unrelated tasks
 * have no reason to start from the same place. */
export interface TaskWorktree {
  branch: string;
  /** True when the branch doesn't exist yet and has to be cut from `baseBranch`. */
  create: boolean;
  baseBranch: string | null;
  accountId: string;
  prompt: string | null;
  /** The board task this row came from, when it came from one. Its body can't
   * ride in `prompt` — that's a single-line shell argument — so the backend
   * writes it into the worktree as `.hangar/task-<key>.md` and points the agent
   * at the file. */
  task: BoardTask | null;
}

/** What a project's git already thinks about a branch you want to work on. */
export interface BranchAvailability {
  branch: string;
  exists: boolean;
  /** Where it's currently checked out — git will refuse to put it anywhere
   * else. Usually the user's own main clone. */
  checkedOutAt: string | null;
}

/** What to type into the shell as soon as it comes up. Only meaningful with
 * an account — "start claude" as nobody in particular isn't a thing Hangar
 * offers, and the backend rejects it. */
export type InitCommand = 'start' | 'login';

export const INIT_COMMAND_LABEL: Record<InitCommand, string> = {
  start: 'start the CLI',
  login: 're-login',
};

/** One chunk of PTY output. Base64 because raw terminal bytes aren't
 * guaranteed to be valid UTF-8 at chunk boundaries — decoding them as text
 * would corrupt any multi-byte character split across two reads. */
export interface TerminalChunk {
  base64: string;
}

/** What a terminal's agent is doing, observed from its output stream — no hook
 * and no configuration of the user's CLI. `waiting` is the one that matters:
 * it means the agent finished its turn and it's your move. */
export type TerminalState = 'working' | 'waiting' | 'idle' | 'exited';

export interface TerminalActivity {
  id: string;
  state: TerminalState;
  /** The last terminal title the program set. Claude Code puts its status line
   * here, which describes the actual task far better than the project name. */
  title: string | null;
}

export const TERMINAL_STATE_META: Record<TerminalState, { label: string; color: string }> = {
  working: { label: 'working', color: 'var(--acc)' },
  waiting: { label: 'waiting for you', color: 'var(--amber)' },
  idle: { label: 'idle', color: 'var(--dim)' },
  exited: { label: 'exited', color: 'var(--dim)' },
};
