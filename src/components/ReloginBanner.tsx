import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { useNavigation } from '../nav/NavigationProvider';
import { Button } from './ui/button';
import { toastError } from './ErrorOverlay';
import type { AccountNeedsLogin } from '../types/account';
import type { TerminalInfo } from '../types/terminal';
import styles from './ReloginBanner.module.css';

/**
 * Says when a provider has expired an account's login, and re-runs it.
 *
 * Providers retire an OAuth session every so often. Before this, Hangar found
 * out the way you did: you opened a terminal, the agent refused to start, and
 * nothing on screen said why or what to do. The background check in `lib.rs`
 * asks every account that *can* be asked, on its own slow cycle, and this is
 * where the answer surfaces.
 *
 * Shaped after `UpdateBanner` on purpose — same bar, same dismiss, same "the
 * app noticed something, you decide what to do about it" contract. It never
 * re-authenticates on its own: that opens a browser and takes over the screen,
 * which is not something to do to someone mid-turn.
 */
export function ReloginBanner() {
  const [accounts, setAccounts] = useState<AccountNeedsLogin[]>([]);
  const [dismissed, setDismissed] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const { navigate } = useNavigation();

  const load = useCallback(() => {
    invoke<AccountNeedsLogin[]>('list_accounts_needing_login')
      .then(setAccounts)
      // Quietly: the table is empty until the first check runs, and a failure
      // to read it is not something to interrupt anyone about.
      .catch(() => {});
  }, []);

  useEffect(() => {
    load();
    // The background cycle emits this when an account's answer changes.
    const unlisten = listen('account-logins-checked', () => load());
    return () => {
      unlisten.then((f) => f());
    };
  }, [load]);

  // A login terminal is how this gets fixed, so its exit is the moment to ask
  // again rather than waiting out the next half-hour cycle.
  useEffect(() => {
    const unlisten = listen('terminal-state', () => load());
    return () => {
      unlisten.then((f) => f());
    };
  }, [load]);

  const pending = accounts.filter((a) => !dismissed.includes(a.accountId));
  if (pending.length === 0) return null;

  const [first] = pending;
  const relogin = async () => {
    setBusy(true);
    try {
      // A real terminal rather than a hidden process: the provider's login
      // prints a URL and waits, and hiding that would leave someone staring at
      // a spinner while the CLI asks a question nobody can see.
      const info = await invoke<TerminalInfo>('open_login_terminal', { accountId: first.accountId, cols: 80, rows: 24 });
      navigate('terminals', { focusTerminalId: info.id });
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.banner}>
      <span className={styles.provider}>{first.provider}</span>
      <span className={styles.text}>
        {pending.length === 1
          ? `${first.name} needs to sign in again — its session with the provider expired.`
          : `${first.name} and ${pending.length - 1} other account${pending.length > 2 ? 's' : ''} need to sign in again.`}
      </span>
      <span className={styles.spacer} />
      <Button size="sm" disabled={busy} onClick={relogin}>
        {busy ? 'opening…' : `Sign in to ${first.name}`}
      </Button>
      {/* Dismissing hides this account until the next launch or the next time
          its answer changes — not forever. The account is still expired, and a
          banner you can permanently silence about something still broken is a
          banner that lies. */}
      <button className={styles.dismiss} onClick={() => setDismissed((prev) => [...prev, first.accountId])}>
        ✕
      </button>
    </div>
  );
}
