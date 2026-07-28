import { useEffect, useState } from 'react';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
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
export function UpdateBanner() {
  const [update, setUpdate] = useState<Update | null>(null);
  const [state, setState] = useState<'idle' | 'installing' | 'done' | 'failed'>('idle');
  const [error, setError] = useState<string | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    // Quietly: no network, an endpoint that isn't there yet, or a build with no
    // updater configured are all normal, and none of them is worth a toast.
    check()
      .then((found) => setUpdate(found))
      .catch(() => {});
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
