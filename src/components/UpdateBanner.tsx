import { useEffect, useState } from 'react';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { invoke } from '../lib/invoke';
import { Button } from './ui/button';
import styles from './UpdateBanner.module.css';

/**
 * Tells you a new version exists, and installs it when you say so.
 *
 * Never on its own: an app that restarts itself in the middle of six live agent
 * terminals is worse than an app that's a version behind. So the check is
 * automatic and the install is a button.
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
  const [update, setUpdate] = useState<Update | null>(null);
  const [state, setState] = useState<'idle' | 'installing' | 'done' | 'failed'>('idle');
  const [error, setError] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    let stopped = false;

    // The releases live in a private repository, so the check carries a token
    // — read from the OS keychain at this moment rather than compiled in, so
    // the binary itself holds no credential and a copy of the app grants
    // nobody anything. No token means no update, which is the right answer for
    // a machine that was never set up for them.
    const look = () => {
      if (stopped) return;
      invoke<Record<string, string> | null>('update_request_headers')
        .then((headers) => check(headers ? { headers } : undefined))
        .then((found) => {
          // Never replace one already on screen: the user may be mid-install,
          // and re-rendering the banner under them would reset it. That same
          // "only on the null→found transition" check is what keeps the OS
          // notification to once per version instead of once per 30-minute poll.
          if (found && !stopped) {
            setUpdate((current) => {
              if (!current) invoke('notify_update_available', { version: found.version }).catch(() => {});
              return current ?? found;
            });
          }
        })
        // Quietly: no network, no token, or an endpoint that has no release yet
        // are all normal, and none of them is worth a toast.
        .catch(() => {});
    };

    look();

    // Checking only at launch meant a version published while you worked was
    // invisible until you happened to restart — which, for an app you leave
    // open all day with six agents in it, is never.
    const timer = window.setInterval(look, CHECK_EVERY_MS);
    // And on coming back to the window: it costs nothing and it's the moment
    // you'd want to know.
    const onFocus = () => look();
    window.addEventListener('focus', onFocus);

    return () => {
      stopped = true;
      window.clearInterval(timer);
      window.removeEventListener('focus', onFocus);
    };
  }, []);

  if (!update || dismissed) return null;

  const install = async () => {
    setState('installing');
    try {
      await update.downloadAndInstall();
      setState('done');
    } catch (e) {
      setError(String(e));
      setState('failed');
    }
  };

  return (
    <div className={styles.banner}>
      <span className={styles.version}>v{update.version}</span>
      <span className={styles.text}>
        {state === 'done'
          ? 'Installed — restart to run it.'
          : state === 'failed'
            ? `Update failed: ${error}`
            : 'A new version of Hangar is available.'}
      </span>
      <span className={styles.spacer} />
      {state === 'done' ? (
        // Restarting is the user's call even here: they may have an agent
        // mid-turn, and the update is already on disk either way.
        <Button size="sm" onClick={() => relaunch()}>
          Restart
        </Button>
      ) : (
        <Button size="sm" disabled={state === 'installing'} onClick={install}>
          {state === 'installing' ? 'installing…' : 'Update'}
        </Button>
      )}
      <button className={styles.dismiss} onClick={() => setDismissed(true)}>
        ✕
      </button>
    </div>
  );
}
