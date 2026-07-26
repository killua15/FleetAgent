import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { useNavigation } from '../nav/NavigationProvider';
import { PROVIDER_META, type Account } from '../types/account';
import type { AuthStatus } from '../types/login';
import { Button } from '../components/ui/button';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import { PickConfigDirDialog } from '../components/PickConfigDirDialog';
import { Spinner } from '../components/Spinner';
import styles from './AccountList.module.css';

/** Hangar's own login wizard parks each login in
 * `<app data>/pending-logins/<uuid>` and keeps it as the account's permanent
 * home, so this path is the signature of an account that isn't using one of
 * the user's own `~/.claude-*` directories — the reason its terminal command
 * shows something unrecognizable. */
function isWizardConfigDir(path: string): boolean {
  return path.includes('/pending-logins/');
}

/** `/Users/killua/.claude-work` → `~/.claude-work`. The home prefix is the
 * same on every row, so it's pure noise in a list; the full path stays in the
 * row's tooltip. macOS-only, like the rest of Hangar. */
function abbreviateHome(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, '~');
}

const STATUS_COLOR: Record<Account['status'], string> = {
  healthy: 'var(--grn)',
  cooling_down: 'var(--amber)',
  exhausted: 'var(--red)',
};

export function AccountList() {
  const { navigate, back, canGoBack } = useNavigation();
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [authByAccount, setAuthByAccount] = useState<Record<string, AuthStatus>>({});
  const [loading, setLoading] = useState(true);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = async () => {
    setLoading(true);
    const result = await invoke<Account[]>('list_accounts');
    setAccounts(result);
    setLoading(false);
    result.forEach((acc) => {
      invoke<AuthStatus>('check_account_auth', { id: acc.id })
        .then((status) => setAuthByAccount((prev) => ({ ...prev, [acc.id]: status })))
        .catch(() => {});
    });
  };

  useEffect(() => {
    load();
  }, []);

  const handleConfirmDelete = async (acc: Account) => {
    setError(null);
    setDeletingId(acc.id);
    try {
      await invoke('delete_account', { id: acc.id });
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setDeletingId(null);
    }
  };

  return (
    <div className={styles.screen}>
      <div className={styles.header}>
        {canGoBack && (
          <Button variant="outline" className={styles.backBtn} onClick={back}>
            ← back
          </Button>
        )}
        <span className={styles.title}>Accounts</span>
        <span className={styles.spacer} />
        <Button className={styles.addBtn} onClick={() => navigate('add-account')}>
          + Add account
        </Button>
      </div>
      <div className={styles.list}>
        {error && (
          <div className={styles.empty} style={{ color: 'var(--red)' }}>
            {error}
          </div>
        )}
        {loading && (
          <div className={styles.empty}>
            <Spinner label="loading accounts…" />
          </div>
        )}
        {!loading && accounts.length === 0 && (
          <div className={styles.empty}>No accounts yet — add your first Claude, Codex or Gemini account.</div>
        )}
        {accounts.map((acc) => {
          const meta = PROVIDER_META[acc.provider];
          return (
            <div className={styles.row} key={acc.id}>
              <span className={styles.badge} style={{ background: meta.tintVar, color: meta.colorVar }}>
                {meta.letter}
              </span>
              <div className={styles.info}>
                <span className={styles.name}>{acc.name}</span>
                <span className={styles.meta} title={acc.configDirPath}>
                  {acc.planLabel}
                  {acc.planLabel && ' · '}
                  <code className={styles.configDir}>{abbreviateHome(acc.configDirPath)}</code>
                </span>
              </div>
              <PickConfigDirDialog
                trigger={
                  <Button
                    variant="outline"
                    className={styles.deleteBtn}
                    // The wizard's opaque uuid folder is the case worth
                    // nudging about; an account already on a recognizable
                    // directory doesn't need a call to action.
                    style={isWizardConfigDir(acc.configDirPath) ? { color: 'var(--amber)', borderColor: 'var(--amber)' } : undefined}
                    title="Point this account at a config directory you already use"
                  >
                    config dir…
                  </Button>
                }
                title={`Config directory for ${acc.name}`}
                description="Pick a login you already have on this machine. Nothing is moved or copied — the account just starts using that directory, and its sessions and history stay intact."
                onPick={async (login) => {
                  await invoke('set_account_config_dir', { id: acc.id, configDirPath: login.path });
                  await load();
                }}
              />
              <span className={styles.scopePill}>{acc.scope}</span>
              {authByAccount[acc.id] && (
                <span
                  className={styles.statusPill}
                  style={{ color: authByAccount[acc.id].loggedIn ? 'var(--grn)' : 'var(--red)' }}
                  title={authByAccount[acc.id].authMethod}
                >
                  {authByAccount[acc.id].loggedIn ? 'signed in' : 'signed out'}
                </span>
              )}
              <span className={styles.statusPill} style={{ color: STATUS_COLOR[acc.status] }}>
                {acc.status.replace('_', ' ')}
              </span>
              <ConfirmDeleteDialog
                trigger={
                  <Button variant="outline" className={styles.deleteBtn} disabled={deletingId === acc.id}>
                    {deletingId === acc.id ? 'deleting…' : 'delete'}
                  </Button>
                }
                title={`Delete ${acc.name}?`}
                description="This removes the account and its credentials from Hangar. This cannot be undone."
                onConfirm={() => handleConfirmDelete(acc)}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
