import type { ReactNode } from 'react';
import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke, useInFlightCount } from '../lib/invoke';
import { useNavigation, type ScreenName } from '../nav/NavigationProvider';
import type { ProjectPrInbox } from '../types/pullRequest';
import { Button } from './ui/button';
import { IconTooltip } from './ui/tooltip';
import { Spinner } from './Spinner';
import { AccountsIcon, BranchIcon, FilesIcon, HomeIcon, PullRequestIcon, SettingsIcon, TasksIcon, TerminalIcon } from './NavIcons';
import { useAuth } from '../auth/AuthProvider';
import styles from './AppTopBar.module.css';

/** One destination: icon, label, active state, optional count.
 *
 * The label is the whole point of the redesign — an icon row you have to learn
 * by trial is one you re-learn every time. And the active one is the *only*
 * coloured item, so colour finally answers "where am I" instead of decorating:
 * before, green meant both Terminals and Queue, and purple meant both Accounts
 * and Tasks. */
function NavItem({ screen, label, icon, badge = 0 }: { screen: ScreenName; label: string; icon: ReactNode; badge?: number }) {
  const { current, navigate } = useNavigation();
  const active = current.screen === screen;
  return (
    <button className={active ? styles.navItemActive : styles.navItem} onClick={() => navigate(screen)} aria-current={active ? 'page' : undefined}>
      {icon}
      <span className={styles.navLabel}>{label}</span>
      {badge > 0 && <span className={styles.navBadge}>{badge > 99 ? '99+' : badge}</span>}
    </button>
  );
}

function countPendingPrComments(inbox: ProjectPrInbox[]): number {
  return inbox.reduce((sum, p) => sum + p.entries.reduce((s, e) => s + e.comments.length, 0), 0);
}

// Below this, a request that resolves quickly never gets a visible indicator
// at all — avoids flicker on the common case (a fast sqlite-backed command)
// while still catching anything that actually takes a moment (a `gh` call, a
// worktree operation).
const BUSY_INDICATOR_DELAY_MS = 400;

/** True only once `count` has stayed above zero continuously for
 * `delayMs` — flips back to false the instant `count` returns to zero. */
function useDelayedBusy(count: number, delayMs: number): boolean {
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (count === 0) {
      setBusy(false);
      return;
    }
    const id = window.setTimeout(() => setBusy(true), delayMs);
    return () => window.clearTimeout(id);
  }, [count, delayMs]);
  return busy;
}

/** Hangar's own account, at the far end of the bar.
 *
 * Signed out it's an invitation, not a wall — everything in the app works
 * without it. Signed in it's how you reach backup and the team plan. */
function AccountButton() {
  const { email, entitlement, signingIn, error, signIn, signOut } = useAuth();

  if (!email) {
    return (
      <IconTooltip label={error ? `Sign-in failed: ${error}` : 'Sign in to back up your setup across machines — everything else works signed out'}>
        <Button variant="outline" size="sm" disabled={signingIn} onClick={signIn} style={error ? { color: 'var(--amber)' } : undefined}>
          {signingIn ? <Spinner label="signing in…" /> : 'Sign in'}
        </Button>
      </IconTooltip>
    );
  }

  return (
    <IconTooltip label={`Signed in as ${email}${entitlement ? ` · ${entitlement.plan}` : ''} — click to sign out`}>
      <Button variant="outline" size="sm" onClick={signOut}>
        {email.split('@')[0]}
        {entitlement && entitlement.plan !== 'free' && <span style={{ marginLeft: 6, color: 'var(--grn)' }}>{entitlement.plan}</span>}
      </Button>
    </IconTooltip>
  );
}

export function AppTopBar({ leading, primaryAction }: { leading?: ReactNode; primaryAction?: ReactNode }) {
  const { navigate, current } = useNavigation();
  const [pendingPrComments, setPendingPrComments] = useState(0);
  const inFlight = useInFlightCount();
  const showBusy = useDelayedBusy(inFlight, BUSY_INDICATOR_DELAY_MS);

  const refreshPrInbox = () => {
    invoke<ProjectPrInbox[]>('list_pr_review_inbox').then((inbox) => setPendingPrComments(countPendingPrComments(inbox)));
  };

  useEffect(() => {
    refreshPrInbox();
    const unlisten = listen('pr-comments-found', refreshPrInbox);
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  return (
    <div className={styles.bar}>
      {/* A screen's own layout controls belong at the far left, ahead of the
       * wordmark — the place every editor puts them, and more importantly a
       * place that doesn't move when the thing they control disappears. */}
      {leading}
      <span className={styles.wordmark}>⌥ hangar</span>
      <span className={styles.version}>v0.1</span>
      {showBusy && (
        <span className={styles.busyIndicator} title="A background request is in progress">
          <Spinner label="working…" />
        </span>
      )}
      <nav className={styles.icons}>
        {/* Home first and alone: it's the only one that isn't a destination so
         * much as a way back. Hidden on Terminals, which *is* home. */}
        {current.screen !== 'terminals' && (
          <IconTooltip label="Back to terminals">
            <button className={styles.navBtn} onClick={() => navigate('terminals')}>
              <HomeIcon />
            </button>
          </IconTooltip>
        )}

        {/* Grouped by what you're doing, with a rule between groups, because a
         * flat row of nine gives no clue that "Git" and "Settings" are
         * different kinds of thing. Work first — it's what you're here for. */}
        <NavItem screen="terminals" label="Terminals" icon={<TerminalIcon />} />
        <NavItem screen="task-board" label="Tasks" icon={<TasksIcon />} />
        <NavItem screen="pr-reviews" label="PRs" icon={<PullRequestIcon />} badge={pendingPrComments} />

        <span className={styles.navRule} />

        <NavItem screen="git-panel" label="Git" icon={<BranchIcon />} />
        <NavItem screen="files" label="Files" icon={<FilesIcon />} />

        <span className={styles.navRule} />

        {/* Icon-only: a person and a gear are the two symbols nobody needs
         * taught, and they're the two you press least. */}
        <IconTooltip label="Accounts — add one or point it at a login">
          <button className={current.screen === 'account-list' ? styles.navBtnActive : styles.navBtn} onClick={() => navigate('account-list')}>
            <AccountsIcon />
          </button>
        </IconTooltip>
        <IconTooltip label="Settings">
          <button
            className={current.screen.endsWith('-settings') ? styles.navBtnActive : styles.navBtn}
            onClick={() => navigate('general-settings')}
          >
            <SettingsIcon />
          </button>
        </IconTooltip>
        {/* The Queue screen is hidden: nothing reaches it now that this entry
         * is gone. The screen, `queued_runs`, and the background `tick_queue`
         * poller are all untouched and still work — restoring the feature is
         * putting a NavItem for `queue` back here. */}
      </nav>
      <span className={styles.spacer} />
      <Button
        variant="outline"
        className={styles.palette}
        title="Command palette"
        onClick={() => navigate('command-palette')}
      >
        search sessions… <span className={styles.kbd}>⌘K</span>
      </Button>
      <AccountButton />
      {primaryAction}
    </div>
  );
}
