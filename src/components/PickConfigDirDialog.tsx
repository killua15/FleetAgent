import { useEffect, useState, type ReactNode } from 'react';
import { invoke } from '../lib/invoke';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DialogTrigger } from './ui/dialog';
import { Spinner } from './Spinner';
import type { DiscoveredLogin } from '../types/account';
import styles from './PickConfigDirDialog.module.css';

/**
 * Picks one of the Claude Code config directories already logged in on this
 * machine — the `~/.claude-work` style folders a user drives by hand with
 * `CLAUDE_CONFIG_DIR=… claude`.
 *
 * Hangar's own wizard parks each login it performs in an opaque
 * `pending-logins/<uuid>` folder, which is why an account created that way
 * shows an unrecognizable path in its terminal command. This is how an
 * account gets pointed at a directory the user actually recognizes.
 */
export function PickConfigDirDialog({
  trigger,
  title,
  description,
  onPick,
}: {
  trigger: ReactNode;
  title: string;
  description: string;
  onPick: (login: DiscoveredLogin) => Promise<void> | void;
}) {
  const [open, setOpen] = useState(false);
  const [logins, setLogins] = useState<DiscoveredLogin[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busyPath, setBusyPath] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    // Loaded on open, not on mount: each candidate costs a real
    // `claude auth status` round-trip, and most renders never open this.
    setLogins(null);
    setError(null);
    invoke<DiscoveredLogin[]>('list_discoverable_logins')
      .then(setLogins)
      .catch((e) => {
        setError(String(e));
        setLogins([]);
      });
  }, [open]);

  const pick = async (login: DiscoveredLogin) => {
    setBusyPath(login.path);
    setError(null);
    try {
      await onPick(login);
      setOpen(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyPath(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>

        {error && <div className={styles.error}>{error}</div>}
        {logins === null && (
          <div className={styles.loading}>
            <Spinner label="checking which logins are still valid…" />
          </div>
        )}
        {logins?.length === 0 && !error && (
          <div className={styles.empty}>
            No unused logins found. Hangar looks for <code>~/.claude*</code> folders that contain a valid login and aren't already
            attached to another account.
          </div>
        )}
        <div className={styles.list}>
          {logins?.map((login) => (
            <button key={login.path} className={styles.row} disabled={busyPath !== null} onClick={() => pick(login)}>
              <span className={styles.path}>{login.displayPath}</span>
              <span className={styles.meta}>
                {login.email ?? 'no email on this login'}
                {login.subscriptionType ? ` · ${login.subscriptionType}` : ''}
              </span>
              {busyPath === login.path && <Spinner label="" />}
            </button>
          ))}
        </div>
      </DialogContent>
    </Dialog>
  );
}
