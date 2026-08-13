export type Provider = 'claude' | 'codex' | 'gemini' | 'kimi' | 'grok';
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

/** An account whose provider login has expired — from
 * `list_accounts_needing_login`, filled by the background check.
 *
 * Only providers with a real status probe can appear here: Gemini, Kimi and
 * Grok have no way to be asked, so they're absent rather than assumed fine. */
export interface AccountNeedsLogin {
  accountId: string;
  name: string;
  provider: Provider;
  /** When the check ran. The banner is claiming something about the world at a
   * moment in time, and on a laptop that was asleep for two days that matters. */
  checkedAt: string;
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
  // ASSUMPTION: letter, color and loginCmd are cosmetic guesses, not
  // verified against a real `grok` CLI — see the comment on providers/mod.rs's
  // "grok" spec() arm for what else about this provider is unverified.
  grok: {
    name: 'Grok',
    cli: 'grok cli',
    letter: '𝕏',
    colorVar: 'var(--grok)',
    tintVar: 'var(--grokD)',
    loginCmd: 'grok auth login',
  },
  // Kimi runs *through* Claude Code, pointed at Moonshot's
  // Anthropic-compatible endpoint — which is why it inherits session ids,
  // resume and transcript transfer instead of needing its own adapter. It has
  // no interactive login: the credential is a Moonshot API key.
  kimi: {
    name: 'Kimi',
    cli: 'claude code → moonshot',
    letter: 'K',
    colorVar: 'var(--teal)',
    tintVar: 'var(--tealD)',
    loginCmd: '',
  },
};

/** Providers that authenticate with an API key rather than an interactive
 * login. The wizard has to skip the login step for these, and offering one
 * would open a terminal running a command that doesn't exist. */
export const KEY_ONLY_PROVIDERS: Provider[] = ['kimi'];

/** Providers whose interactive login ends by asking for a code pasted back.
 *
 * Claude prints a URL and waits on stdin. Codex runs a loopback OAuth flow that
 * finishes on its own when the browser redirects — showing it a paste box is
 * asking for something that will never exist, which is exactly what made a
 * completed Codex login look stuck. */
export const LOGIN_EXPECTS_CODE: Provider[] = ['claude'];

/** One quota window, forecast from the readings the usage poller has been
 * taking all along. `hoursLeft` is null when nothing is being burned — a flat
 * line means "not running out", not "runs out now". */
export interface Forecast {
  pct: number;
  burnPerHour: number | null;
  hoursLeft: number | null;
  samples: number;
}

export interface AccountQuota {
  accountId: string;
  accountName: string;
  status: AccountStatus;
  /** The rolling 5-hour window — the one you recover from over lunch. */
  session: Forecast | null;
  /** The weekly window — the one you don't recover from, and so the only one
   * worth planning around. */
  weekly: Forecast | null;
}

export interface FanOutProjection {
  accountId: string;
  accountName: string;
  hoursLeft: number | null;
  limitingWindow: 'session' | 'weekly' | 'none';
  exhausted: boolean;
}

/** "2 h 10 m", "45 m", or "—" when there's nothing to run out of. */
export function formatHoursLeft(hours: number | null): string {
  if (hours == null) return '—';
  if (hours < 1) return `${Math.max(1, Math.round(hours * 60))} m`;
  const h = Math.floor(hours);
  const m = Math.round((hours - h) * 60);
  return m === 0 ? `${h} h` : `${h} h ${m} m`;
}
