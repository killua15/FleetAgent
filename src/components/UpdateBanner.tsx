import { useEffect, useState } from 'react';
import { relaunch } from '@tauri-apps/plugin-process';
import { runUpdateCheck, useUpdateStatus } from '../lib/updates';
import { Button } from './ui/button';
import styles from './UpdateBanner.module.css';

/**
 * Keeps Hangar up to date on its own and says when a restart would finish it.
 *
 * Checking, downloading and installing all happen in the background
 * (`runUpdateCheck`); the one thing left to the user is the restart. That
 * stays theirs: an app that restarts itself in the middle of six live agent
 * terminals is worse than one that's a version behind — and the new build is
 * already on disk, so the next launch runs it either way.
 *
 * The updater signs with its own keypair, which is why this works on a build
 * Apple never signed — Gatekeeper is paid once, at the first install, instead
 * of on every fix.
 */
/** Half an hour. Often enough that a release published mid-morning reaches you
 * before lunch, rare enough that it's invisible next to what this app already
 * does over the network. */
const CHECK_EVERY_MS = 30 * 60 * 1000;

export function UpdateBanner() {
  const status = useUpdateStatus();
  const [dismissedFor, setDismissedFor] = useState<string | null>(null);

  useEffect(() => {
    // Quietly: no network, no session, or no newer build are all normal. A
    // real failure lands in the status (and in `hangar.log`), not a toast.
    const look = () => void runUpdateCheck();
    look();
    // Checking only at launch meant a version published while you worked was
    // invisible until you happened to restart — which, for an app you leave
    // open all day with six agents in it, is never.
    const timer = window.setInterval(look, CHECK_EVERY_MS);
    // And on coming back to the window: it costs nothing and it's the moment
    // you'd want to know.
    window.addEventListener('focus', look);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener('focus', look);
    };
  }, []);

  if (status.phase === 'installing') {
    return (
      <div className={styles.banner}>
        <span className={styles.version}>v{status.version}</span>
        <span className={styles.text}>Downloading the new version…</span>
      </div>
    );
  }

  if (status.phase === 'ready' && dismissedFor !== status.version) {
    return (
      <div className={styles.banner}>
        <span className={styles.version}>v{status.version}</span>
        <span className={styles.text}>Installed — restart Hangar to run it.</span>
        <span className={styles.spacer} />
        {/* Restarting is the user's call: they may have an agent mid-turn. */}
        <Button size="sm" onClick={() => relaunch()}>
          Restart
        </Button>
        <button className={styles.dismiss} onClick={() => setDismissedFor(status.version)}>
          ✕
        </button>
      </div>
    );
  }

  if (status.phase === 'failed' && status.version && dismissedFor !== status.version) {
    return (
      <div className={styles.banner}>
        <span className={styles.version}>v{status.version}</span>
        <span className={styles.text}>Update failed: {status.error}</span>
        <span className={styles.spacer} />
        <Button size="sm" onClick={() => void runUpdateCheck()}>
          Retry
        </Button>
        <button className={styles.dismiss} onClick={() => setDismissedFor(status.version)}>
          ✕
        </button>
      </div>
    );
  }

  return null;
}
