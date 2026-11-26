import type { ReactNode } from 'react';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke, useInFlightCount } from '../lib/invoke';
import { useNavigation, type ScreenName } from '../nav/NavigationProvider';
import type { ProjectPrInbox } from '../types/pullRequest';
import { IconTooltip } from './ui/tooltip';
import { Spinner } from './Spinner';
import { useSlidingIndicator } from '../lib/motion';
import { fitCount } from '../lib/navOverflow';
import { AccountsIcon, BellIcon, BranchIcon, CompassIcon, FactoryIcon, FilesIcon, HomeIcon, MachinesIcon, MenuIcon, PullRequestIcon, QueueIcon, SettingsIcon, TasksIcon, TerminalIcon, UsageIcon } from './NavIcons';
import { UsageModal } from './UsageModal';
import { NotificationBell, useUnreadNotifications } from './NotificationBell';
import { registerTopBar, toggleNotifications } from '../lib/notifications';
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
    <button className={active ? styles.navItemActive : styles.navItem} onClick={() => navigate(screen)} aria-current={active ? 'page' : undefined} title={label}>
      {icon}
      <span className={styles.navLabel}>{label}</span>
      {badge > 0 && <span className={styles.navBadge}>{badge > 99 ? '99+' : badge}</span>}
    </button>
  );
}

type NavEntry = { kind: 'item'; screen: ScreenName; label: string; icon: ReactNode; badge?: number } | { kind: 'rule' };

/** What the trailing "More" button costs when it has to appear — measured
 * from the live element once rendered; this covers the first pass. */
const MORE_FALLBACK_WIDTH = 44;

/**
 * Talmo's `tnav`: the destinations in one raised pill, with a marker that
 * slides to whichever one you're on.
 *
 * When the bar is too narrow for all of them, the ones that don't fit — from
 * the end — move into a "More" dropdown at the end of the pill rather than
 * being clipped. Widths come from an invisible copy of every item, so they
 * stay right as labels collapse at breakpoints and as badges come and go.
 */
function NavPill({ lead, entries }: { lead?: ReactNode; entries: NavEntry[] }) {
  const { current, navigate } = useNavigation();
  const slotRef = useRef<HTMLDivElement>(null);
  const navRef = useRef<HTMLElement>(null);
  const pill = useRef<HTMLSpanElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const moreRef = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(entries.length);

  const signature = entries.map((e) => (e.kind === 'item' ? `${e.screen}:${e.badge ?? 0}` : '|')).join(',');

  useLayoutEffect(() => {
    const slot = slotRef.current;
    const measure = measureRef.current;
    if (!slot || !measure) return;
    const recompute = () => {
      // Hidden below the tab-bar breakpoint: nothing to fit.
      if (slot.offsetParent === null) return;
      const nodes = Array.from(measure.children) as HTMLElement[];
      const leadEl = nodes[0];
      const itemNodes = nodes.slice(1);
      const widths = itemNodes.map((n) => n.offsetWidth);
      const kinds = entries.map((e) => e.kind);
      const gap = 2;
      // The pill's own padding (4px a side) plus the home button when shown.
      const chrome = 8 + (lead && leadEl ? leadEl.offsetWidth + gap : 0);
      const more = moreRef.current?.offsetWidth || MORE_FALLBACK_WIDTH;
      setVisible(fitCount(widths, kinds, slot.clientWidth, chrome, gap, more));
    };
    recompute();
    const ro = new ResizeObserver(recompute);
    ro.observe(slot);
    ro.observe(measure);
    return () => ro.disconnect();
    // `signature` stands in for `entries`, which is a new array every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [signature, lead !== undefined]);

  const shown = entries.slice(0, visible);
  const hidden = entries.slice(visible).filter((e): e is Extract<NavEntry, { kind: 'item' }> => e.kind === 'item');
  const hiddenActive = hidden.some((e) => e.screen === current.screen);
  const hiddenBadges = hidden.reduce((sum, e) => sum + (e.badge ?? 0), 0);

  useSlidingIndicator(navRef, pill, `${current.screen}:${visible}`);

  const render = (e: NavEntry, i: number) =>
    e.kind === 'rule' ? <span key={`rule-${i}`} className={styles.navRule} /> : <NavItem key={e.screen} screen={e.screen} label={e.label} icon={e.icon} badge={e.badge} />;

  return (
    <div ref={slotRef} className={styles.navSlot}>
      <nav ref={navRef} className={styles.icons}>
        <span ref={pill} className={styles.navPill} aria-hidden />
        {lead}
        {shown.map(render)}
        {hidden.length > 0 && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                ref={moreRef}
                className={hiddenActive ? styles.navItemActive : styles.navItem}
                data-active={hiddenActive ? 'true' : undefined}
                aria-label={`More — ${hidden.map((e) => e.label).join(', ')}`}
                title="More"
              >
                <MenuIcon />
                {hiddenBadges > 0 && <span className={styles.navBadge}>{hiddenBadges > 99 ? '99+' : hiddenBadges}</span>}
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {hidden.map((e) => (
                <DropdownMenuItem
                  key={e.screen}
                  onClick={() => navigate(e.screen)}
                  className={e.screen === current.screen ? styles.moreItemActive : undefined}
                >
                  {e.icon} {e.label}
                  {(e.badge ?? 0) > 0 && <span className={styles.moreCount}>{e.badge}</span>}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </nav>
      {/* An invisible copy of every item at its natural width — what the fit
       * is computed from, since the real ones may not be rendered at all. */}
      <div ref={measureRef} className={styles.navMeasure} aria-hidden inert>
        <span>{lead}</span>
        {entries.map((e, i) =>
          e.kind === 'rule' ? (
            <span key={`m-rule-${i}`} className={styles.navRule} />
          ) : (
            <span key={`m-${e.screen}`} className={styles.navItem}>
              {e.icon}
              <span className={styles.navLabel}>{e.label}</span>
              {(e.badge ?? 0) > 0 && <span className={styles.navBadge}>{(e.badge ?? 0) > 99 ? '99+' : e.badge}</span>}
            </span>
          ),
        )}
      </div>
    </div>
  );
}

function TabItem({ screen, label, icon, badge = 0 }: { screen: ScreenName; label: string; icon: ReactNode; badge?: number }) {
  const { current, navigate } = useNavigation();
  const active = current.screen === screen;
  return (
    <button className={styles.tab} onClick={() => navigate(screen)} aria-current={active ? 'page' : undefined}>
      <span className={styles.tabIco}>
        {icon}
        {badge > 0 && <span className={styles.tabCount}>{badge > 99 ? '99+' : badge}</span>}
      </span>
      {label}
    </button>
  );
}

function TabBar({ children }: { children: ReactNode }) {
  const { current } = useNavigation();
  const ref = useRef<HTMLElement>(null);
  const pill = useRef<HTMLSpanElement>(null);
  useSlidingIndicator(ref, pill, current.screen);
  return (
    <nav ref={ref} className={styles.tabbar} data-tabbar>
      <span ref={pill} className={styles.tabPill} aria-hidden />
      {children}
    </nav>
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
  // While a bar is on screen the bell lives in it; screens without one get a
  // floating bell instead (see `App.tsx`).
  useEffect(() => registerTopBar(), []);
  const unreadNotifications = useUnreadNotifications();
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
    <>
    <div className={styles.bar}>
      {/* A screen's own layout controls belong at the far left, ahead of the
       * wordmark — the place every editor puts them, and more importantly a
       * place that doesn't move when the thing they control disappears. */}
      {leading}
      <span className={styles.brand}>
        <span className={styles.mark} aria-hidden>
          ⌥
        </span>
        <span className={styles.brandText}>
          <b className={styles.wordmark}>hangar</b>
          <small className={styles.version}>v0.1</small>
        </span>
      </span>
      {showBusy && (
        <span className={styles.busyIndicator} title="A background request is in progress">
          <Spinner label="working…" />
        </span>
      )}
      <NavPill
        lead={
          /* Home first and alone: it's the only one that isn't a destination so
           * much as a way back. Hidden on Terminals, which *is* home. */
          current.screen !== 'terminals' ? (
            <IconTooltip label="Back to terminals">
              <button className={styles.navBtn} onClick={() => navigate('terminals')}>
                <HomeIcon />
              </button>
            </IconTooltip>
          ) : undefined
        }
        /* Grouped by what you're doing, with a rule between groups, because a
         * flat row of nine gives no clue that "Git" and "Settings" are
         * different kinds of thing. Work first — it's what you're here for,
         * and it's also what stays when the bar is too narrow for everything:
         * the tail of this list is what moves into More first. */
        entries={[
          { kind: 'item', screen: 'start-here', label: 'Empieza aquí', icon: <CompassIcon />, badge: stepsLeft },
          { kind: 'item', screen: 'terminals', label: 'Terminals', icon: <TerminalIcon /> },
          // The terminals of your other machines, next to this one's.
          { kind: 'item', screen: 'machines', label: 'Machines', icon: <MachinesIcon /> },
          { kind: 'item', screen: 'task-board', label: 'Tasks', icon: <TasksIcon /> },
          // Next to Tasks because it answers the same question — what is going
          // to run — for work that's already been handed over and is only
          // waiting on quota.
          { kind: 'item', screen: 'queue', label: 'Queue', icon: <QueueIcon /> },
          { kind: 'item', screen: 'pr-reviews', label: 'PRs', icon: <PullRequestIcon />, badge: pendingPrComments },
          { kind: 'rule' },
          // A group of its own: the line runs work end to end on its own,
          // which is a different kind of thing from the screens you drive.
          { kind: 'item', screen: 'factory', label: 'Factory', icon: <FactoryIcon /> },
          { kind: 'rule' },
          { kind: 'item', screen: 'git-panel', label: 'Git', icon: <BranchIcon /> },
          { kind: 'item', screen: 'files', label: 'Files', icon: <FilesIcon /> },
        ]}
      />

      {/* Talmo's round icon buttons: things you open rather than places you
       * go, so they sit outside the pill and its sliding marker. */}
      <div className={styles.tools}>
        {/* A modal, not a screen: it answers a question and hands you back to
            what you were doing. Navigating away would close the terminals you
            were watching while you checked. */}
        <IconTooltip label="Usage — what each account has spent, this window and this week">
          <button className={usageOpen ? styles.navBtnActive : styles.navBtn} onClick={() => setUsageOpen(true)} aria-label="Usage">
            <UsageIcon />
          </button>
        </IconTooltip>
        {/* What Hangar notified about, readable even when Do Not Disturb kept
         * the OS notifications away. */}
        <NotificationBell className={styles.navBtn} activeClassName={styles.navBtnActive} />
        {/* Icon-only: a person and a gear are the two symbols nobody needs
         * taught, and they're the two you press least. */}
        <IconTooltip label="Accounts — add one or point it at a login">
          <button className={current.screen === 'account-list' ? styles.navBtnActive : styles.navBtn} onClick={() => navigate('account-list')} aria-label="Accounts">
            <AccountsIcon />
          </button>
        </IconTooltip>
        <IconTooltip label="Settings">
          <button
            className={current.screen.endsWith('-settings') ? styles.navBtnActive : styles.navBtn}
            onClick={() => navigate('general-settings')}
            aria-label="Settings"
          >
            <SettingsIcon />
          </button>
        </IconTooltip>
      </div>
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
    {/* Talmo's tab bar: below 760px the pill no longer fits, so the five
     * destinations you use most move to the bottom edge, and everything —
     * same destinations, same order as before — stays one tap away in More. */}
    <TabBar>
      <TabItem screen="terminals" label="Terminals" icon={<TerminalIcon />} />
      <TabItem screen="machines" label="Machines" icon={<MachinesIcon />} />
      <TabItem screen="task-board" label="Tasks" icon={<TasksIcon />} />
      <TabItem screen="pr-reviews" label="PRs" icon={<PullRequestIcon />} badge={pendingPrComments} />
      <TabItem screen="factory" label="Factory" icon={<FactoryIcon />} />
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button className={styles.tab} aria-label="Menu">
            <span className={styles.tabIco}>
              <MenuIcon />
              {pendingPrComments + unreadNotifications > 0 && (
                <span className={styles.tabCount}>{pendingPrComments + unreadNotifications > 99 ? '99+' : pendingPrComments + unreadNotifications}</span>
              )}
            </span>
            More
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent side="top">
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
          <DropdownMenuItem onClick={() => navigate('machines')}>
            <MachinesIcon /> Machines
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('task-board')}>
            <TasksIcon /> Tasks
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('queue')}>
            <QueueIcon /> Queue
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('pr-reviews')}>
            <PullRequestIcon /> PRs{pendingPrComments > 0 && ` (${pendingPrComments})`}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('factory')}>
            <FactoryIcon /> Factory
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
          <DropdownMenuItem onClick={toggleNotifications}>
            <BellIcon /> Notifications{unreadNotifications > 0 && ` (${unreadNotifications})`}
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('account-list')}>
            <AccountsIcon /> Accounts
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => navigate('general-settings')}>
            <SettingsIcon /> Settings
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </TabBar>
    </>
  );
}
