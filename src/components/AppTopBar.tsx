import type { ReactNode } from 'react';
import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke, useInFlightCount } from '../lib/invoke';
import { useNavigation } from '../nav/NavigationProvider';
import type { ProjectPrInbox } from '../types/pullRequest';
import { Button } from './ui/button';
import { IconTooltip } from './ui/tooltip';
import { Spinner } from './Spinner';
import styles from './AppTopBar.module.css';

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

export function AppTopBar({ primaryAction }: { primaryAction?: ReactNode }) {
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
      <span className={styles.wordmark}>⌥ hangar</span>
      <span className={styles.version}>v0.1</span>
      {showBusy && (
        <span className={styles.busyIndicator} title="A background request is in progress">
          <Spinner label="working…" />
        </span>
      )}
      <div className={styles.icons}>
        {/* Terminals is the home now, so it needs no "back home" affordance;
         * every other screen on this bar does. Jumps straight there rather
         * than using `back()`, same reasoning as SettingsTabs: unwinding one
         * history entry usually lands on another screen of the same feature. */}
        {current.screen !== 'terminals' && (
          <IconTooltip label="Back to terminals">
            <Button variant="outline" className={styles.iconBtn} style={{ color: 'var(--mut)' }} onClick={() => navigate('terminals')}>
              ⌂
            </Button>
          </IconTooltip>
        )}
        {/* Accounts used to be reachable only from MainView's sidebar; with
         * MainView retired as the home, the top bar carries the entry. */}
        <IconTooltip label="Accounts — add one or point it at a login">
          <Button variant="outline" className={styles.iconBtn} style={{ color: 'var(--acc)' }} onClick={() => navigate('account-list')}>
            ◐
          </Button>
        </IconTooltip>
        {/* Hierarchy trees is hidden for now — the whole agent-tree feature is
         * parked while its design is reconsidered. Re-adding this entry (and
         * the Settings toggle) is all it takes to bring it back. */}
        <IconTooltip label="Terminals — open a Claude Code shell in a project">
          <Button variant="outline" className={styles.iconBtn} style={{ color: 'var(--grn)' }} onClick={() => navigate('terminals')}>
            ⌗
          </Button>
        </IconTooltip>
        <IconTooltip label="Git — repos and worktrees overview">
          <Button variant="outline" className={styles.iconBtn} style={{ color: 'var(--teal)' }} onClick={() => navigate('git-panel')}>
            ⎇
          </Button>
        </IconTooltip>
        <div className={styles.iconBtnWrap}>
          <IconTooltip label="PR reviews — open PRs with new review comments">
            <Button variant="outline" className={styles.iconBtn} style={{ color: 'var(--amber)' }} onClick={() => navigate('pr-reviews')}>
              💬
            </Button>
          </IconTooltip>
          {pendingPrComments > 0 && <span className={styles.iconBadge}>{pendingPrComments > 99 ? '99+' : pendingPrComments}</span>}
        </div>
        <IconTooltip label="Settings">
          <Button variant="outline" className={styles.iconBtn} style={{ color: 'var(--mut)' }} onClick={() => navigate('general-settings')}>
            ⚙
          </Button>
        </IconTooltip>
      </div>
      <span className={styles.spacer} />
      <Button
        variant="outline"
        className={styles.palette}
        title="Command palette"
        onClick={() => navigate('command-palette')}
      >
        search sessions… <span className={styles.kbd}>⌘K</span>
      </Button>
      {primaryAction}
    </div>
  );
}
