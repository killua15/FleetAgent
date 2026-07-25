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
  /** The Claude Code session id Fleet started this terminal on, when it's an
   * account-scoped claude terminal — what makes the conversation resumable or
   * transferable later. `null` for a plain shell / login / non-claude. */
  sessionUuid: string | null;
}

/** The account the out-of-quota banner offers to reopen a terminal under. */
export interface SuggestedAccount {
  accountId: string;
  accountName: string;
}

/** What to type into the shell as soon as it comes up. Only meaningful with
 * an account — "start claude" as nobody in particular isn't a thing Fleet
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
