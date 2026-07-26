export type Provider = 'claude' | 'codex' | 'gemini';
export type Billing = 'sub' | 'key';
export type Scope = 'work' | 'personal';
export type AccountStatus = 'healthy' | 'cooling_down' | 'exhausted';

export interface Account {
  id: string;
  provider: Provider;
  billing: Billing;
  name: string;
  scope: Scope;
  planLabel: string;
  configDirPath: string;
  credentialRef: string;
  monthlyCap: number | null;
  status: AccountStatus;
  createdAt: string;
  updatedAt: string;
}

/** Login state for one account, from the batch `list_account_auth` command.
 * `supported` is false for providers Hangar can't scope a process to yet
 * (codex, gemini) — they can be added, but nothing can run as them. */
export interface AccountAuth {
  accountId: string;
  loggedIn: boolean;
  authMethod: string;
  supported: boolean;
  /** The exact command a terminal for this account will run, e.g.
   * `CLAUDE_CONFIG_DIR=/Users/me/.claude-work claude`. Comes from the same
   * backend function that executes it, so what the row shows can't drift from
   * what happens. `null` when there's nothing runnable. */
  command: string | null;
}

export interface NewAccount {
  provider: Provider;
  billing: Billing;
  name: string;
  scope: Scope;
  planLabel: string;
  secret?: string;
  loginId?: string;
  /** A config dir already on disk and already logged in (e.g.
   * `~/.claude-work`), adopted in place instead of running a fresh login. */
  configDirPath?: string;
  monthlyCap?: number;
}

/** A Claude Code config directory already logged in on this machine that no
 * Hangar account is using yet — the ones a user drives by hand with
 * `CLAUDE_CONFIG_DIR=~/.claude-work claude`. */
export interface DiscoveredLogin {
  path: string;
  /** `~`-relative when under the home directory, which is where these live. */
  displayPath: string;
  email: string | null;
  subscriptionType: string | null;
}

export const PROVIDER_META: Record<
  Provider,
  { name: string; cli: string; letter: string; colorVar: string; tintVar: string; loginCmd: string }
> = {
  claude: {
    name: 'Claude',
    cli: 'claude code',
    letter: 'C',
    colorVar: 'var(--acc)',
    tintVar: 'var(--accD)',
    loginCmd: 'claude auth login',
  },
  codex: {
    name: 'Codex',
    cli: 'codex cli',
    letter: 'X',
    colorVar: 'var(--teal)',
    tintVar: 'var(--tealD)',
    loginCmd: 'codex login',
  },
  gemini: {
    name: 'Gemini',
    cli: 'gemini cli',
    letter: 'G',
    colorVar: 'var(--gemini)',
    tintVar: 'var(--geminiD)',
    loginCmd: 'gemini auth',
  },
};
