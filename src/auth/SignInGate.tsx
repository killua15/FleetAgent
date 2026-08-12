import type { ReactNode } from 'react';
import { Spinner } from '../components/Spinner';
import { useAuth } from './AuthProvider';
import styles from './SignInGate.module.css';

/** Google's mark, inline: the CSP has no external hosts, and this is four paths. */
function GoogleMark() {
  return (
    <svg width="16" height="16" viewBox="0 0 48 48" aria-hidden="true">
      <path fill="#4285F4" d="M45.1 24.5c0-1.6-.1-3.2-.4-4.7H24v8.9h11.8c-.5 2.7-2 5-4.4 6.6v5.5h7.1c4.1-3.8 6.6-9.4 6.6-16.3z" />
      <path fill="#34A853" d="M24 46c5.9 0 10.9-2 14.5-5.2l-7.1-5.5c-2 1.3-4.5 2.1-7.4 2.1-5.7 0-10.5-3.8-12.2-9H4.5v5.7C8.1 41.3 15.5 46 24 46z" />
      <path fill="#FBBC05" d="M11.8 28.4c-.4-1.3-.7-2.7-.7-4.4s.3-3.1.7-4.4v-5.7H4.5A22 22 0 0 0 2 24c0 3.6.9 6.9 2.5 9.9l7.3-5.5z" />
      <path fill="#EA4335" d="M24 10.4c3.2 0 6.1 1.1 8.4 3.3l6.3-6.3C34.9 3.9 29.9 2 24 2 15.5 2 8.1 6.7 4.5 13.9l7.3 5.7c1.7-5.2 6.5-9.2 12.2-9.2z" />
    </svg>
  );
}

function Shell({ children }: { children: ReactNode }) {
  return (
    <div className={styles.gate}>
      <div className={styles.content}>
        <div className={styles.mark}>⌥</div>
        {children}
      </div>
    </div>
  );
}

/**
 * The front door. Nothing behind it renders until the account checks out.
 *
 * It sits inside the app shell rather than replacing it, so the update banner
 * stays reachable: someone locked out by a version too old to pass the gate
 * has to be able to see and take the update that fixes it.
 */
export function SignInGate({ children }: { children: ReactNode }) {
  const { access, email, signingIn, error, signIn, signOut, recheck } = useAuth();

  if (access === 'ready') return <>{children}</>;

  if (access === 'checking') {
    return (
      <Shell>
        <Spinner label="checking your account…" />
      </Shell>
    );
  }

  if (access === 'pending') {
    return (
      <Shell>
        <div className={styles.heading}>
          <span className={styles.wordmark}>Almost in</span>
          <span className={styles.blurb}>
            Your account is waiting to be approved. Once it is, this screen turns into Hangar —
            no need to sign in again.
          </span>
        </div>
        {email && <span className={styles.email}>{email}</span>}
        <div className={styles.row}>
          <button type="button" className={styles.secondaryBtn} onClick={() => void recheck()}>
            Check again
          </button>
          <button type="button" className={styles.secondaryBtn} onClick={() => void signOut()}>
            Use another account
          </button>
        </div>
      </Shell>
    );
  }

  if (access === 'grace-expired') {
    return (
      <Shell>
        <div className={styles.heading}>
          <span className={styles.wordmark}>Offline too long</span>
          <span className={styles.blurb}>
            Hangar runs offline for a week between check-ins, and that week is up. Connect to the
            internet once and it picks up where it left off — nothing has been signed out.
          </span>
        </div>
        <div className={styles.row}>
          <button type="button" className={styles.secondaryBtn} onClick={() => void recheck()}>
            Try again
          </button>
        </div>
      </Shell>
    );
  }

  return (
    <Shell>
      <div className={styles.heading}>
        <span className={styles.wordmark}>hangar</span>
        <span className={styles.blurb}>
          Mission control for your coding agents. Sign in with Google to continue — it's how
          Hangar knows whose machine this is.
        </span>
      </div>

      <button type="button" className={styles.googleBtn} disabled={signingIn} onClick={() => void signIn()}>
        {signingIn ? <Spinner label="waiting for the browser…" /> : (<><GoogleMark /> Continue with Google</>)}
      </button>

      {error && <span className={styles.error}>Sign-in failed: {error}</span>}

      <span className={styles.footNote}>
        Sign-in opens your normal browser · your CLI accounts and keys never leave this machine
      </span>
    </Shell>
  );
}
