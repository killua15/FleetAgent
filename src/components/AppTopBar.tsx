import type { ReactNode } from 'react';
import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke, useInFlightCount } from '../lib/invoke';
import { useNavigation, type ScreenName } from '../nav/NavigationProvider';
import type { ProjectPrInbox } from '../types/pullRequest';
import { IconTooltip } from './ui/tooltip';
import { Spinner } from './Spinner';
import { AccountsIcon, BranchIcon, CompassIcon, FilesIcon, HomeIcon, MenuIcon, PullRequestIcon, SettingsIcon, TasksIcon, TerminalIcon, UsageIcon } from './NavIcons';
import { UsageModal } from './UsageModal';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from './ui/dropdown-menu';
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

export function AppTopBar({ leading, primaryAction }: { leading?: ReactNode; primaryAction?: ReactNode }) {
  const { navigate, current } = useNavigation();
  const [pendingPrComments, setPendingPrComments] = useState(0);
  const [usageOpen, setUsageOpen] = useState(false);
  // Visible mientras esté activo, no solo en Settings: alguien pone yolo para
  // una prueba y se le queda puesto.
  const [mode, setMode] = useState<string>('ask_edits');
  const inFlight = useInFlightCount();
  const showBusy = useDelayedBusy(inFlight, BUSY_INDICATOR_DELAY_MS);

  // Cuántos pasos esenciales quedan, para el contador de "Empieza aquí".
  // Se pide al montar y no en cada navegación: cambia cuando el usuario hace
  // algo, no cuando cambia de pantalla, y el propio badge desaparece solo al
  // llegar a cero.
  const [stepsLeft, setStepsLeft] = useState(0);

  const refreshPrInbox = () => {
    invoke<ProjectPrInbox[]>('list_pr_review_inbox').then((inbox) => setPendingPrComments(countPendingPrComments(inbox)));
  };

  useEffect(() => {
    invoke<string>('get_default_terminal_mode').then(setMode).catch(() => {});
  }, [current.screen]);

  useEffect(() => {
    invoke<{ basicsDone: number; basicsTotal: number }>('onboarding_progress')
      .then((p) => setStepsLeft(p.basicsTotal - p.basicsDone))
      .catch(() => {});
  }, []);

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
        <NavItem screen="start-here" label="Empieza aquí" icon={<CompassIcon />} badge={stepsLeft} />
        <NavItem screen="terminals" label="Terminals" icon={<TerminalIcon />} />
        <NavItem screen="task-board" label="Tasks" icon={<TasksIcon />} />
        <NavItem screen="pr-reviews" label="PRs" icon={<PullRequestIcon />} badge={pendingPrComments} />

        <span className={styles.navRule} />

        <NavItem screen="git-panel" label="Git" icon={<BranchIcon />} />
        <NavItem screen="files" label="Files" icon={<FilesIcon />} />
        {/* A modal, not a screen: it answers a question and hands you back to
            what you were doing. Navigating away would close the terminals you
            were watching while you checked. */}
        <IconTooltip label="Usage — what each account has spent, this window and this week">
          <button className={usageOpen ? styles.navItemActive : styles.navItem} onClick={() => setUsageOpen(true)}>
            <UsageIcon />
            <span className={styles.navLabel}>Usage</span>
          </button>
        </IconTooltip>

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
      {/* Below `.navMenu`'s breakpoint the row above overlaps rather than
       * wrapping (no flex-wrap, and every item already at its minimum
       * width), so instead of shrinking further it collapses into this one
       * button — same destinations, same order, as a dropdown. */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button className={styles.navMenuTrigger} aria-label="Menu">
            <MenuIcon />
            {pendingPrComments > 0 && <span className={styles.navBadge}>{pendingPrComments > 99 ? '99+' : pendingPrComments}</span>}
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent>
          {current.screen !== 'terminals' && (
            <DropdownMenuItem onClick={() => navigate('terminals')}>
              <HomeIcon /> Back to terminals
            </DropdownMenuItem>
          )}
          <DropdownMenuItem onClick={() => navigate('start-here')}>
            <CompassIcon /> Empieza aquí{stepsLeft > 0 && ` (${stepsLeft})`}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('terminals')}>
            <TerminalIcon /> Terminals
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('task-board')}>
            <TasksIcon /> Tasks
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('pr-reviews')}>
            <PullRequestIcon /> PRs{pendingPrComments > 0 && ` (${pendingPrComments})`}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('git-panel')}>
            <BranchIcon /> Git
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('files')}>
            <FilesIcon /> Files
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => setUsageOpen(true)}>
            <UsageIcon /> Usage
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('account-list')}>
            <AccountsIcon /> Accounts
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('general-settings')}>
            <SettingsIcon /> Settings
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <span className={styles.spacer} />
      {mode === 'yolo' && (
        <span className={styles.yoloBadge} title="Las terminales de este Mac ejecutan sin pedir permiso — Settings → modo de las terminales">
          yolo
        </span>
      )}
      {/* The ⌘K command palette is hidden: it searches `list_workspace_sessions`
       * rows that never get pruned when a terminal closes, so closed ones
       * lingered forever — and the sidebar's own filter already covers the
       * same "find a terminal" job against live data. `CommandPalette.tsx`,
       * its route, and the ⌘K shortcut in `App.tsx` are untouched; restoring
       * this is putting the button back and fixing the underlying query. */}
      {/* Who is signed in used to sit here. It lives in Settings → general now,
       * with the plan and the way out — the bar is for where you're going, not
       * for who you are. */}
      {primaryAction}
      {usageOpen && <UsageModal onClose={() => setUsageOpen(false)} />}
    </div>
  );
}
