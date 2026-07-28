import { useEffect, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { AccountSwitcher } from '../components/AccountSwitcher';
import { TerminalPane } from '../components/TerminalPane';
import { toastError } from '../components/ErrorOverlay';
import { Button } from '../components/ui/button';
import { IconTooltip } from '../components/ui/tooltip';
import { BaseBranchPicker, useBaseBranch } from '../components/BaseBranchPicker';
import { useNavigation } from '../nav/NavigationProvider';
import { autoGridColumns } from '../lib/gridLayout';
import { formatHoursLeft, PROVIDER_META, type Account, type AccountAuth, type FanOutProjection } from '../types/account';
import type { Project, Session } from '../types/session';
import type { BoardTask, TaskFetch, TaskSourceLink } from '../types/task';
import { TERMINAL_STATE_META, type BranchAvailability, type FanOutBranch, type TaskWorktree, type InitCommand, type SuggestedAccount, type TerminalActivity, type TerminalInfo, type TerminalTarget } from '../types/terminal';
import styles from './Terminals.module.css';

/** Terminals opened past this point stop being supervisable at a glance —
 * same threshold and same reasoning as the agent grid's own warning. */
const WARN_THRESHOLD = 4;

/** The "no account" row. A plain shell is still worth one click — it's what
 * the terminals were before they knew about accounts — but it sits below the
 * real ones because picking an account is the point of this list. */
const PLAIN_SHELL = '__plain__';

export function Terminals() {
  const { current, navigate } = useNavigation();
  const openFor = current.params?.openFor as TerminalTarget | undefined;
  const openForAccountId = current.params?.accountId as string | undefined;
  const focusTerminalId = current.params?.focusTerminalId as string | undefined;

  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [auth, setAuth] = useState<Record<string, AccountAuth>>({});
  const [authLoading, setAuthLoading] = useState(false);
  // The picker is two steps: pick the account, then pick where it runs. `null`
  // means we're on step one. The account leads because that's the decision
  // that actually varies — which AI you're talking to — while the directory
  // is usually whichever project you were already thinking about.
  const [pickedAccountId, setPickedAccountId] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [gridColumns, setGridColumns] = useState<number | 'auto'>('auto');
  const [pickerOpen, setPickerOpen] = useState(false);
  const [multiOpen, setMultiOpen] = useState(false);
  const [fanOpen, setFanOpen] = useState(false);
  const [tasksOpen, setTasksOpen] = useState(false);
  const [opening, setOpening] = useState(false);
  // The project a "⑃ new worktree…" is being named for — `null` when that
  // sub-step isn't open. Two steps rather than one because the name matters:
  // it becomes a real branch.
  const [worktreeFor, setWorktreeFor] = useState<Project | null>(null);
  // Which panel is mid-account-switch. Per terminal rather than the global
  // `opening`: the swap takes a few seconds (a transcript copy and a new CLI
  // coming up) and the feedback belongs on the panel it's happening to.
  const [switchingId, setSwitchingId] = useState<string | null>(null);
  // Live agent state per terminal, keyed by id. Seeded once and then driven by
  // events — polling six PTYs for "are you done" would be the wrong shape.
  const [activity, setActivity] = useState<Record<string, TerminalActivity>>({});
  // The panel the sidebar last sent you to, highlighted briefly so a click on a
  // row of twelve lands somewhere visible instead of scrolling silently.
  const [focusedId, setFocusedId] = useState<string | null>(null);
  // Remembered across launches: whether the sidebar is worth its 260px is a
  // standing preference, not a per-session mood.
  const [sidebarOpen, setSidebarOpen] = useState(() => window.localStorage.getItem('hangar.sidebar') !== 'closed');
  // Navigating here with an `openFor` param must open exactly one terminal,
  // not one per render — and not another one if the user navigates back to
  // this screen later with the same param still in the history entry.
  const handledOpenForRef = useRef(false);

  // `restore_terminals` reopens whatever was open when Hangar last closed on the
  // first mount of this run (and resumes each conversation), and just returns
  // the live list on later mounts — so navigating away and back never
  // re-restores. 80x24 is the shell's initial size; each pane's first fit()
  // corrects it.
  const loadTerminals = () => invoke<TerminalInfo[]>('restore_terminals', { cols: 80, rows: 24 }).then(setTerminals);

  const loadProjects = () => invoke<Project[]>('list_projects').then(setProjects);

  const loadAccounts = () => invoke<Account[]>('list_accounts').then(setAccounts);

  useEffect(() => {
    Promise.all([loadTerminals(), loadProjects(), loadAccounts()]).catch((e) => toastError(String(e)));
  }, []);

  // Agent state: one snapshot for the first render, then the event stream.
  useEffect(() => {
    invoke<TerminalActivity[]>('list_terminal_states')
      .then((list) => setActivity(Object.fromEntries(list.map((a) => [a.id, a]))))
      .catch(() => {});
    const unlisten = listen<TerminalActivity>('terminal-state', (event) => {
      setActivity((prev) => ({ ...prev, [event.payload.id]: event.payload }));
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  // The usage poller flips an account to `exhausted` when it runs out of quota;
  // reloading accounts re-evaluates every panel's out-of-quota banner.
  useEffect(() => {
    const unlisten = listen('terminal-accounts-updated', () => {
      loadAccounts().catch(() => {});
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  /** Opens the macOS folder dialog and turns the chosen directory into a
   * project (creating it, or reusing an existing row for that path). Returns
   * the project, or null if the user cancelled. Adds it to the recents list so
   * it shows up immediately in both pickers. */
  const exploreForProject = async (): Promise<Project | null> => {
    const picked = await open({ directory: true, multiple: false, title: 'Choose a project folder' });
    if (typeof picked !== 'string') return null;
    const project = await invoke<Project>('ensure_project_for_path', { path: picked });
    setProjects((prev) => (prev.some((p) => p.id === project.id) ? prev : [...prev, project]));
    return project;
  };

  /** Loads each account's login state once, lazily — every account picker
   * needs it (ready vs. will-log-in, and the exact command shown), but it
   * costs a real `claude auth status` per account, so it waits for the first
   * picker to open rather than running on mount. */
  const ensureAuthLoaded = () => {
    if (authLoading || Object.keys(auth).length > 0) return;
    setAuthLoading(true);
    invoke<AccountAuth[]>('list_account_auth')
      .then((list) => setAuth(Object.fromEntries(list.map((a) => [a.accountId, a]))))
      .catch((e) => toastError(String(e)))
      .finally(() => setAuthLoading(false));
  };

  const openTerminal = async (target: TerminalTarget, accountId?: string, initCommand?: InitCommand) => {
    setOpening(true);
    try {
      // 80x24 is the classic default the shell starts against; the pane's
      // first `fit()` replaces it with the real size a moment later.
      const info = await invoke<TerminalInfo>('open_terminal', {
        target,
        accountId: accountId ?? null,
        init: initCommand ?? null,
        // Hangar mints the session id itself for a normal open.
        sessionUuid: null,
        cols: 80,
        rows: 24,
      });
      setTerminals((prev) => [...prev, info]);
      setPickerOpen(false);
      setPickedAccountId(null);
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };

  // Opening from a session card: inherit that session's account so the shell
  // is authenticated as the agent working there, and start its CLI.
  useEffect(() => {
    if (!openFor || handledOpenForRef.current) return;
    handledOpenForRef.current = true;
    openTerminal(openFor, openForAccountId, openForAccountId ? 'start' : undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openFor]);

  const openPicker = () => {
    setMultiOpen(false);
    setFanOpen(false);
    setPickerOpen((wasOpen) => !wasOpen);
    setPickedAccountId(null);
    setWorktreeFor(null);
    if (!pickerOpen) ensureAuthLoaded();
  };

  const openMultiPicker = () => {
    setPickerOpen(false);
    setFanOpen(false);
    setMultiOpen((wasOpen) => !wasOpen);
    if (!multiOpen) ensureAuthLoaded();
  };

  const openFanPicker = () => {
    setPickerOpen(false);
    setMultiOpen(false);
    setTasksOpen(false);
    setFanOpen((wasOpen) => !wasOpen);
    if (!fanOpen) ensureAuthLoaded();
  };

  const openTasksPicker = () => {
    setPickerOpen(false);
    setMultiOpen(false);
    setFanOpen(false);
    setTasksOpen((wasOpen) => !wasOpen);
    if (!tasksOpen) ensureAuthLoaded();
  };

  /** Opens one worktree and one agent per task, each on the task's own branch.
   * The opposite of a fan-out: N different jobs, all of which you keep. */
  const runTasks = async (projectId: string, tasks: TaskWorktree[]) => {
    setOpening(true);
    try {
      const infos = await invoke<TerminalInfo[]>('open_task_worktrees', { projectId, tasks, cols: 80, rows: 24 });
      setTerminals((prev) => [...prev, ...infos]);
      setTasksOpen(false);
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };

  /** Creates a real git worktree for `project` (its own branch, `.env` copied,
   * setup command run) and opens the terminal inside it. The worktree outlives
   * the terminal on purpose — closing a panel must never take uncommitted work
   * with it; removing it lives in the Git panel. */
  const openWorktreeTerminal = async (project: Project, name: string, setupCommand: string, baseBranch: string) => {
    if (pickedAccountId === null || pickedAccountId === PLAIN_SHELL) return;
    setOpening(true);
    try {
      const session = await invoke<Session>('create_worktree_for_project', {
        projectId: project.id,
        name,
        accountId: pickedAccountId,
        setupCommand: setupCommand.trim() || null,
        baseBranch: baseBranch || null,
      });
      await openTerminal({ kind: 'session', id: session.id }, pickedAccountId, initFor(pickedAccountId));
      setWorktreeFor(null);
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };

  const runFanOut = async (projectId: string, branches: FanOutBranch[], prompt: string, baseBranch: string) => {
    setOpening(true);
    try {
      const infos = await invoke<TerminalInfo[]>('fan_out_prompt', { projectId, branches, prompt, baseBranch, cols: 80, rows: 24 });
      setTerminals((prev) => [...prev, ...infos]);
      setFanOpen(false);
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };

  /** The init command an account needs: its CLI when authenticated, its login
   * when not — the latter being the only way in Hangar to recover an account
   * whose token expired. `undefined` for the plain-shell pseudo-account. */
  const initFor = (accountId: string): InitCommand | undefined => {
    if (accountId === PLAIN_SHELL) return undefined;
    return auth[accountId]?.loggedIn ? 'start' : 'login';
  };

  /** One click on a target finishes the job: open the shell in that directory
   * under the chosen account, running the right command. */
  const openForTarget = (target: TerminalTarget) => {
    if (pickedAccountId === null) return;
    openTerminal(target, pickedAccountId === PLAIN_SHELL ? undefined : pickedAccountId, initFor(pickedAccountId));
  };

  const exploreAndOpen = async () => {
    try {
      const project = await exploreForProject();
      if (project) openForTarget({ kind: 'project', id: project.id });
    } catch (e) {
      toastError(String(e));
    }
  };

  /** Sidebar → panel. Un-hides it if it was hidden, then scrolls it into view:
   * a row that does nothing because its panel is collapsed is a dead control. */
  const focusTerminal = (id: string) => {
    setCollapsed((prev) => (prev[id] ? { ...prev, [id]: false } : prev));
    setFocusedId(id);
    // After the panel has had a frame to mount if it was hidden.
    requestAnimationFrame(() => {
      document.getElementById(`terminal-card-${id}`)?.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    });
  };

  useEffect(() => {
    window.localStorage.setItem('hangar.sidebar', sidebarOpen ? 'open' : 'closed');
  }, [sidebarOpen]);

  // ⌘B toggles it, the convention every editor with a sidebar uses. Ignored
  // while typing in a field so it can't fire from the filter box or a rename.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== 'b') return;
      const el = document.activeElement;
      if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) return;
      e.preventDefault();
      setSidebarOpen((wasOpen) => !wasOpen);
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, []);

  // ⌘J landed here asking for a specific panel. Waits for the terminal list so
  // the scroll target exists — jumping before the grid renders does nothing.
  useEffect(() => {
    if (!focusTerminalId || !terminals.some((t) => t.id === focusTerminalId)) return;
    focusTerminal(focusTerminalId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [focusTerminalId, terminals.length]);

  const renameTerminal = async (id: string, name: string) => {
    try {
      await invoke('rename_terminal', { id, name });
      setTerminals((prev) => prev.map((t) => (t.id === id ? { ...t, displayName: name.trim() || null } : t)));
    } catch (e) {
      toastError(String(e));
    }
  };

  const closeTerminal = async (id: string) => {
    try {
      await invoke('close_terminal', { id });
      setTerminals((prev) => prev.filter((t) => t.id !== id));
    } catch (e) {
      toastError(String(e));
    }
  };

  const reopenWithAccount = async (terminal: TerminalInfo, accountId: string) => {
    setSwitchingId(terminal.id);
    try {
      const info = await invoke<TerminalInfo>('reopen_terminal_with_account', { id: terminal.id, accountId, cols: 80, rows: 24 });
      // The backend closed the old one and opened the new; swap it in place so
      // the panel keeps its spot in the grid.
      setTerminals((prev) => prev.map((t) => (t.id === terminal.id ? info : t)));
    } catch (e) {
      toastError(String(e));
    } finally {
      setSwitchingId(null);
    }
  };

  const exhaustedAccountIds = new Set(accounts.filter((a) => a.status === 'exhausted').map((a) => a.id));

  // Hidden terminals no longer need their own list: the sidebar shows every
  // terminal and marks which ones have a panel on screen.
  const panels = terminals.filter((t) => !collapsed[t.id]);
  const effectiveGridColumns = gridColumns === 'auto' ? autoGridColumns(panels.length) : gridColumns;

  const pickedAccount = accounts.find((a) => a.id === pickedAccountId);
  const pickedAuth = pickedAccountId ? auth[pickedAccountId] : undefined;

  return (
    <div className={styles.screen}>
      <AppTopBar
        leading={
          terminals.length > 0 ? (
            <IconTooltip label={sidebarOpen ? 'Hide the fleet sidebar  ⌘B' : 'Show the fleet sidebar  ⌘B'}>
              <Button
                variant="outline"
                className={styles.sidebarToggle}
                aria-pressed={sidebarOpen}
                onClick={() => setSidebarOpen((wasOpen) => !wasOpen)}
              >
                {/* The icon is the state, not the action: a filled left column
                    means the sidebar is there. An icon showing what would
                    happen instead flips meaning every click, which is the
                    thing that makes these toggles unreadable. */}
                <span className={sidebarOpen ? styles.sidebarGlyphOn : styles.sidebarGlyphOff} />
              </Button>
            </IconTooltip>
          ) : undefined
        }
        primaryAction={
          <div style={{ display: 'flex', gap: 8 }}>
            <Button size="sm" variant="outline" onClick={openTasksPicker} disabled={opening}>
              ☰ Tasks
            </Button>
            <Button size="sm" variant="outline" onClick={openFanPicker} disabled={opening}>
              ⑃ Fan out
            </Button>
            <Button size="sm" variant="outline" onClick={openMultiPicker} disabled={opening}>
              + Multi terminal
            </Button>
            <Button size="sm" onClick={openPicker} disabled={opening}>
              {opening ? '…' : '+ New terminal'}
            </Button>
          </div>
        }
      />

      {pickerOpen && pickedAccountId === null && (
        <div className={styles.picker}>
          <span className={styles.pickerLabel}>
            WHICH ACCOUNT{authLoading && <span className={styles.pickerChecking}> · checking logins…</span>}
          </span>
          <div className={styles.pickerGroup}>
            {accounts.map((a) => {
              const meta = PROVIDER_META[a.provider];
              const status = auth[a.id];
              const unsupported = status && !status.supported;
              return (
                <button
                  key={a.id}
                  className={styles.accountRow}
                  disabled={unsupported}
                  title={unsupported ? `Hangar can't run a ${meta.name} CLI as a specific account yet` : undefined}
                  onClick={() => setPickedAccountId(a.id)}
                >
                  <span className={styles.provBadge} style={{ background: meta.tintVar, color: meta.colorVar }}>
                    {meta.letter}
                  </span>
                  <span className={styles.accountMain}>
                    <span className={styles.accountTop}>
                      <span className={styles.pickerName}>{a.name}</span>
                      <span className={styles.pickerPath}>{meta.name}</span>
                      <span className={styles.spacer} />
                      {/* Until the check lands there's nothing honest to claim,
                          so the row says nothing rather than guessing "ready". */}
                      {!status && <span className={styles.statusChecking}>…</span>}
                      {status && status.supported && status.loggedIn && <span className={styles.statusReady}>✓ ready</span>}
                      {status && status.supported && !status.loggedIn && <span className={styles.statusNeedsLogin}>✗ will log in</span>}
                      {unsupported && <span className={styles.statusChecking}>not supported yet</span>}
                    </span>
                    {/* The literal line that gets typed into the shell. Built
                        by the same backend function that runs it, so this is a
                        promise the app can actually keep. */}
                    {status?.command && <code className={styles.accountCommand}>{status.command}</code>}
                  </span>
                </button>
              );
            })}
            {accounts.length === 0 && <span className={styles.pickerEmpty}>no provider accounts yet — add one below to run its CLI</span>}
            <button className={styles.accountRow} onClick={() => setPickedAccountId(PLAIN_SHELL)}>
              <span className={styles.provBadge} style={{ background: 'var(--panel2)', color: 'var(--dim)' }}>
                $
              </span>
              <span className={styles.pickerName}>Plain shell</span>
              <span className={styles.pickerPath}>no account, nothing started</span>
            </button>
            <button className={styles.exploreRow} onClick={() => navigate('add-account')}>
              <span>＋ Add a provider account…</span>
            </button>
          </div>
        </div>
      )}

      {pickerOpen && pickedAccountId !== null && (
        <div className={styles.picker}>
          <div className={styles.envRow}>
            <button className={styles.backLink} onClick={() => setPickedAccountId(null)}>
              ‹ accounts
            </button>
            <span className={styles.pickerLabel}>
              {pickedAccount ? `${pickedAccount.name} — WHERE SHOULD IT RUN?` : 'WHERE SHOULD IT RUN?'}
            </span>
            {pickedAuth?.command && <code className={styles.accountCommand}>{pickedAuth.command}</code>}
            <span className={styles.spacer} />
            {pickedAuth && !pickedAuth.loggedIn && (
              <span className={styles.envWarn}>this account isn't logged in — the terminal will run its login instead</span>
            )}
            {pickedAccount?.billing === 'key' && (
              // Not a detail to leave implicit: the key is readable by `env`
              // and by anything the user runs in this shell.
              <span className={styles.envWarn}>⚠ this account's API key will be visible in the shell's environment</span>
            )}
          </div>
          {worktreeFor ? (
            <NewWorktreeForm
              project={worktreeFor}
              busy={opening}
              onCancel={() => setWorktreeFor(null)}
              onCreate={(name, setupCommand, baseBranch) => openWorktreeTerminal(worktreeFor, name, setupCommand, baseBranch)}
            />
          ) : (
            <div className={styles.pickerGroup}>
              <span className={styles.pickerGroupTitle}>RECENT PROJECTS</span>
              {projects.map((p) => (
                <div key={p.id} className={styles.projectRowWrap}>
                  <button className={styles.pickerRow} onClick={() => openForTarget({ kind: 'project', id: p.id })}>
                    <span className={styles.pickerName}>{p.name}</span>
                    <span className={styles.pickerPath}>{p.path}</span>
                  </button>
                  {/* A worktree needs an account to run under, so it isn't
                      offered for the plain shell. */}
                  {pickedAccountId !== PLAIN_SHELL && (
                    <button
                      className={styles.worktreeLink}
                      title="Open this terminal in its own git worktree — a separate branch and working directory, so it can't collide with your main checkout"
                      onClick={() => setWorktreeFor(p)}
                    >
                      ⑃ new worktree…
                    </button>
                  )}
                </div>
              ))}
              <button className={styles.exploreRow} onClick={exploreAndOpen}>
                <span>📁 Explore another folder…</span>
              </button>
            </div>
          )}
        </div>
      )}

      {tasksOpen && <TaskPicker projects={projects} accounts={accounts} busy={opening} onClose={() => setTasksOpen(false)} onLaunch={runTasks} />}

      {fanOpen && <FanOutPicker projects={projects} accounts={accounts} auth={auth} busy={opening} onClose={() => setFanOpen(false)} onLaunch={runFanOut} />}

      {multiOpen && (
        <MultiTerminalPicker
          projects={projects}
          accounts={accounts}
          auth={auth}
          authLoading={authLoading}
          onExplore={exploreForProject}
          onClose={() => setMultiOpen(false)}
          onOpenAll={async (rows) => {
            for (const row of rows) {
              // Serial, not parallel: several accounts sharing a config dir
              // must not race, and even distinct ones spawning at once would
              // stampede the machine. A failure in one is surfaced but doesn't
              // abort the rest.
              try {
                await openTerminal({ kind: 'project', id: row.projectId }, row.accountId === PLAIN_SHELL ? undefined : row.accountId, initFor(row.accountId));
              } catch (e) {
                toastError(String(e));
              }
            }
            setMultiOpen(false);
          }}
        />
      )}

      <div className={styles.body}>
        {terminals.length > 0 && sidebarOpen && (
          <FleetSidebar
            terminals={terminals}
            projects={projects}
            activity={activity}
            collapsed={collapsed}
            focusedId={focusedId}
            onFocus={focusTerminal}
            onToggleVisible={(id) => setCollapsed((prev) => ({ ...prev, [id]: !prev[id] }))}
            onRename={renameTerminal}
            onClose={closeTerminal}
          />
        )}

        <div className={styles.gridArea}>
          {panels.length >= WARN_THRESHOLD && (
            <div className={styles.warnBanner}>
              <span className={styles.warnDot} />
              <span>{panels.length} panels on screen — hide some from the sidebar; they keep running either way</span>
              <span className={styles.spacer} />
              <label className={styles.colControl}>
                columns
                <select
                  value={String(gridColumns)}
                  onChange={(e) => setGridColumns(e.target.value === 'auto' ? 'auto' : Number(e.target.value))}
                >
                  <option value="auto">auto</option>
                  {[1, 2, 3, 4].map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          )}

          {panels.length === 0 ? (
            <div className={styles.empty}>
              <span>no terminals open</span>
              <span>use “+ New terminal” above to open a Claude Code shell in a project under one of your accounts</span>
            </div>
          ) : (
            <div className={styles.grid} style={{ gridTemplateColumns: `repeat(${effectiveGridColumns}, minmax(0, 1fr))` }}>
              {panels.map((t) => (
                <div key={t.id} id={`terminal-card-${t.id}`} className={focusedId === t.id ? styles.cardFocused : styles.card}>
                  <div className={styles.cardHeader}>
                    <span className={styles.kindBadge} style={{ background: t.kind === 'session' ? 'var(--tealD)' : 'var(--accD)', color: t.kind === 'session' ? 'var(--teal)' : 'var(--acc)' }}>
                      {t.kind === 'session' ? '⑃' : '▣'}
                    </span>
                    <StateDot activity={activity[t.id]} />
                    <span className={styles.cardTitle}>{t.displayName ?? t.title}</span>
                    {/* What the agent says it's doing beats what the project is
                        called — it's the only label that changes as work moves. */}
                    {activity[t.id]?.title && <span className={styles.agentTitle}>{activity[t.id].title}</span>}
                    {/* An account-scoped panel's chip is the account switcher:
                        the project's rotation pool, one click, conversation
                        carried over. A plain shell has no account to switch. */}
                    {t.accountName && t.accountId && (
                      <AccountSwitcher
                        terminal={t}
                        auth={auth}
                        onEnsureAuth={ensureAuthLoaded}
                        switching={switchingId === t.id}
                        onSwitch={(accountId) => reopenWithAccount(t, accountId)}
                        onManagePool={() => navigate('rotation-settings')}
                      />
                    )}
                    {t.accountName && !t.accountId && (
                      <span className={styles.accountBadge} title={`Running with ${t.accountName}'s credentials`}>
                        {t.accountName}
                      </span>
                    )}
                    <span className={styles.cardPath} title={t.cwd}>
                      {t.cwd}
                    </span>
                    <span className={styles.spacer} />
                    <Button
                      variant="outline"
                      size="icon-sm"
                      title="Collapse to rail — the shell keeps running"
                      onClick={() => setCollapsed((prev) => ({ ...prev, [t.id]: true }))}
                    >
                      ▁
                    </Button>
                    <Button variant="ghost" size="icon" title="Close terminal — kills the shell" style={{ color: 'var(--dim)' }} onClick={() => closeTerminal(t.id)}>
                      ×
                    </Button>
                  </div>
                  {t.accountId && exhaustedAccountIds.has(t.accountId) && (
                    <OutOfQuotaBanner terminal={t} busy={switchingId === t.id} onReopen={(accountId) => reopenWithAccount(t, accountId)} />
                  )}
                  {/* Keyed by id so React never recycles one pane's xterm
                      instance for a different terminal. */}
                  <TerminalPane terminalId={t.id} className={styles.term} />
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** Shown on a panel whose account has run out of quota. Asks the backend which
 * allowed account to offer, and reopens the terminal there on click. For a
 * claude→claude switch the conversation is carried over (Hangar copies the
 * transcript into the new account and resumes it), so it keeps going rather
 * than starting fresh. */
function OutOfQuotaBanner({ terminal, busy, onReopen }: { terminal: TerminalInfo; busy: boolean; onReopen: (accountId: string) => void }) {
  const [suggestion, setSuggestion] = useState<SuggestedAccount | null>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    invoke<SuggestedAccount | null>('suggest_terminal_account', {
      kind: terminal.kind,
      sourceId: terminal.sourceId,
      exhaustedAccountId: terminal.accountId,
    })
      .then(setSuggestion)
      .catch(() => setSuggestion(null))
      .finally(() => setLoaded(true));
  }, [terminal.kind, terminal.sourceId, terminal.accountId]);

  // Only a terminal Hangar started with a session id can carry its conversation.
  const canKeepConversation = terminal.sessionUuid != null;

  return (
    <div className={styles.quotaBanner}>
      <span className={styles.quotaDot} />
      <span className={styles.quotaText}>
        {terminal.accountName ?? 'This account'} is out of quota.
        {suggestion
          ? canKeepConversation
            ? ' Reopening keeps the conversation.'
            : ' Reopening starts a fresh conversation.'
          : loaded
            ? ' No other allowed account to switch to.'
            : ''}
      </span>
      <span className={styles.spacer} />
      {suggestion && (
        <Button size="sm" disabled={busy} onClick={() => onReopen(suggestion.accountId)}>
          {busy ? '…' : `Reopen in ${suggestion.accountName}`}
        </Button>
      )}
    </div>
  );
}

/** The index of everything running, grouped by project.
 *
 * Replaces a rail that only listed *minimized* terminals — which meant the
 * moment you had more panels than fit, the app's answer was a banner telling you
 * to hide some. A sidebar makes many terminals navigable instead of a problem,
 * which is the shape every comparable tool converged on.
 *
 * **Sorted by who needs you**, not by when it was opened: `waiting` first, then
 * working, then the rest. That ordering is the entire reason B1 (agent state)
 * had to come first — without a state there's nothing to sort by, and a list of
 * twelve identical rows is no better than twelve panels.
 */
function FleetSidebar({
  terminals,
  projects,
  activity,
  collapsed,
  focusedId,
  onFocus,
  onToggleVisible,
  onRename,
  onClose,
}: {
  terminals: TerminalInfo[];
  projects: Project[];
  activity: Record<string, TerminalActivity>;
  collapsed: Record<string, boolean>;
  focusedId: string | null;
  onFocus: (id: string) => void;
  onToggleVisible: (id: string) => void;
  onRename: (id: string, name: string) => void;
  onClose: (id: string) => void;
}) {
  const [filter, setFilter] = useState('');
  const [renaming, setRenaming] = useState<string | null>(null);
  const [draft, setDraft] = useState('');

  const q = filter.trim().toLowerCase();
  const label = (t: TerminalInfo) => t.displayName ?? t.title;
  const matches = (t: TerminalInfo) =>
    !q || label(t).toLowerCase().includes(q) || t.cwd.toLowerCase().includes(q) || (t.accountName ?? '').toLowerCase().includes(q) || (activity[t.id]?.title ?? '').toLowerCase().includes(q);

  // Attention first. A terminal that stopped and is waiting is the only kind
  // that costs you something by going unnoticed.
  const rank = (t: TerminalInfo) => {
    switch (activity[t.id]?.state) {
      case 'waiting':
        return 0;
      case 'working':
        return 1;
      case 'exited':
        return 3;
      default:
        return 2;
    }
  };

  const visible = terminals.filter(matches);
  const groups = projects
    .map((p) => ({ project: p, rows: visible.filter((t) => t.cwd === p.path || t.cwd.startsWith(`${p.path}/`)).sort((a, b) => rank(a) - rank(b)) }))
    .filter((g) => g.rows.length > 0);
  // A terminal whose cwd doesn't sit under any known project — an explored
  // folder — still has to appear somewhere.
  const grouped = new Set(groups.flatMap((g) => g.rows.map((r) => r.id)));
  const loose = visible.filter((t) => !grouped.has(t.id)).sort((a, b) => rank(a) - rank(b));

  const waiting = terminals.filter((t) => activity[t.id]?.state === 'waiting').length;

  const renderRow = (t: TerminalInfo) => {
    const state = activity[t.id]?.state ?? 'idle';
    const hidden = Boolean(collapsed[t.id]);
    return (
      <div key={t.id} className={focusedId === t.id ? styles.sideRowActive : styles.sideRow}>
        <button className={styles.sideRowMain} onClick={() => onFocus(t.id)} title={`${t.cwd}\n${TERMINAL_STATE_META[state].label}`}>
          <StateDot activity={activity[t.id]} />
          {renaming === t.id ? (
            <input
              autoFocus
              className={styles.renameInput}
              value={draft}
              onClick={(e) => e.stopPropagation()}
              onChange={(e) => setDraft(e.target.value)}
              onBlur={() => {
                onRename(t.id, draft);
                setRenaming(null);
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  onRename(t.id, draft);
                  setRenaming(null);
                }
                if (e.key === 'Escape') setRenaming(null);
              }}
            />
          ) : (
            <span
              /* Unread is bold, not badged: a badge shouts a number you then
                 have to interpret; weight tells you where to look. */
              className={state === 'waiting' ? styles.sideNameAttention : styles.sideName}
              onDoubleClick={(e) => {
                e.stopPropagation();
                setDraft(t.displayName ?? '');
                setRenaming(t.id);
              }}
            >
              {label(t)}
            </span>
          )}
          {activity[t.id]?.title && <span className={styles.sideAgentTitle}>{activity[t.id].title}</span>}
        </button>
        <button className={styles.sideIconBtn} title={hidden ? 'Show this panel' : 'Hide the panel — the shell keeps running'} onClick={() => onToggleVisible(t.id)}>
          {hidden ? '○' : '●'}
        </button>
        <button className={styles.sideIconBtn} title="Close terminal — kills the shell" onClick={() => onClose(t.id)}>
          ×
        </button>
      </div>
    );
  };

  return (
    <div className={styles.sidebar}>
      <div className={styles.sideHeader}>
        <span className={styles.railLabel}>FLEET</span>
        {waiting > 0 && <span className={styles.sideWaiting}>{waiting} waiting</span>}
      </div>
      <input className={styles.sideFilter} placeholder="filter…" value={filter} onChange={(e) => setFilter(e.target.value)} />
      {groups.map((g) => (
        <div key={g.project.id} className={styles.sideGroup}>
          <span className={styles.sideGroupTitle}>{g.project.name}</span>
          {g.rows.map(renderRow)}
        </div>
      ))}
      {loose.length > 0 && (
        <div className={styles.sideGroup}>
          <span className={styles.sideGroupTitle}>OTHER FOLDERS</span>
          {loose.map(renderRow)}
        </div>
      )}
      {visible.length === 0 && <span className={styles.pickerEmpty}>nothing matches “{filter}”</span>}
    </div>
  );
}

/** The one thing you need at a glance with six panels open: which agent is
 * working, and which has stopped and is waiting for you. */
function StateDot({ activity }: { activity?: TerminalActivity }) {
  const state = activity?.state ?? 'idle';
  const meta = TERMINAL_STATE_META[state];
  return (
    <span
      className={state === 'working' ? styles.stateDotPulse : styles.stateDot}
      style={{ background: meta.color }}
      title={activity?.title ? `${meta.label} — ${activity.title}` : meta.label}
    />
  );
}

/** Names the branch for a terminal that gets its own worktree. Two fields, both
 * consequential: the name becomes a real git branch (`hangar/<slug>`), and the
 * setup command is what makes the worktree usable — a fresh checkout has no
 * `node_modules`. */
function NewWorktreeForm({
  project,
  busy,
  onCancel,
  onCreate,
}: {
  project: Project;
  busy: boolean;
  onCancel: () => void;
  onCreate: (name: string, setupCommand: string, baseBranch: string) => void;
}) {
  const [name, setName] = useState('');
  const [setupCommand, setSetupCommand] = useState('');
  const [baseBranch, setBaseBranch] = useBaseBranch(project.path);
  const slug = name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');

  return (
    <div className={styles.pickerGroup}>
      <span className={styles.pickerGroupTitle}>NEW WORKTREE IN {project.name.toUpperCase()}</span>
      <div className={styles.worktreeForm}>
        <input
          autoFocus
          className={styles.worktreeInput}
          placeholder="what are you working on — e.g. fix billing rounding"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => e.key === 'Enter' && slug && !busy && baseBranch.selected && onCreate(name, setupCommand, baseBranch.selected)}
        />
        <input
          className={styles.worktreeInput}
          placeholder="setup command, optional — e.g. npm install"
          value={setupCommand}
          onChange={(e) => setSetupCommand(e.target.value)}
        />
        <BaseBranchPicker projectPath={project.path} state={baseBranch} onChange={setBaseBranch} />
        <span className={styles.worktreeHint}>
          {slug ? (
            <>
              branch <code>hangar/{slug}</code> off <code>{baseBranch.selected || '…'}</code> · own directory in{' '}
              <code>.hangar/worktrees/{slug}</code> · your main checkout is untouched
            </>
          ) : (
            'a real git worktree: its own branch and directory, with your .env files copied in'
          )}
        </span>
        <div className={styles.worktreeActions}>
          <Button size="sm" variant="outline" onClick={onCancel}>
            cancel
          </Button>
          <Button size="sm" disabled={!slug || busy || !baseBranch.selected} onClick={() => onCreate(name, setupCommand, baseBranch.selected)}>
            {busy ? 'creating…' : 'Create & open'}
          </Button>
        </div>
      </div>
    </div>
  );
}

interface TaskRow {
  id: string;
  branch: string;
  accountId: string;
  prompt: string;
  baseBranch: string;
  /** Set when the row came from a board rather than being typed. Carries the
   * body to the backend, which writes it into the worktree — a task body has
   * newlines and the prompt is a single-line shell argument. */
  task?: BoardTask;
}

const newTaskRow = (): TaskRow => ({ id: crypto.randomUUID(), branch: '', accountId: '', prompt: '', baseBranch: '' });

const rowFromTask = (task: BoardTask): TaskRow => ({
  id: crypto.randomUUID(),
  branch: task.suggestedBranch,
  accountId: '',
  // Left empty on purpose: an empty prompt means "use the brief", and anything
  // typed here wins over it.
  prompt: '',
  baseBranch: '',
  task,
});

/**
 * Several tasks at once, each on its own real branch.
 *
 * Looks like the fan-out and means the opposite. A fan-out is N attempts at one
 * job: shared base, namespaced branches, keep the winner, throw the rest away.
 * This is N different jobs: each on the branch the task already has (or a new
 * one named after it), each with its own prompt, and you keep all of them.
 *
 * Which is why the base is chosen per row — two unrelated tasks have no reason
 * to start from the same commit — and why the branch name is never decorated.
 */
function TaskPicker({
  projects,
  accounts,
  busy,
  onClose,
  onLaunch,
}: {
  projects: Project[];
  accounts: Account[];
  busy: boolean;
  onClose: () => void;
  onLaunch: (projectId: string, tasks: TaskWorktree[]) => void;
}) {
  const [projectId, setProjectId] = useState('');
  const [rows, setRows] = useState<TaskRow[]>([newTaskRow(), newTaskRow()]);
  const [availability, setAvailability] = useState<Record<string, BranchAvailability>>({});
  const project = projects.find((p) => p.id === projectId);
  const [baseBranch] = useBaseBranch(project?.path);
  const [links, setLinks] = useState<TaskSourceLink[]>([]);
  const [board, setBoard] = useState<TaskFetch | null>(null);
  const [loadingBoard, setLoadingBoard] = useState(false);

  // Which sources this project is wired to. Nothing configured is the normal
  // first state, and it must read as "not set up yet" rather than as an error.
  useEffect(() => {
    setBoard(null);
    if (!projectId) {
      setLinks([]);
      return;
    }
    invoke<TaskSourceLink[]>('list_task_source_links', { projectId })
      .then(setLinks)
      .catch(() => setLinks([]));
  }, [projectId]);

  const loadBoard = async () => {
    setLoadingBoard(true);
    try {
      setBoard(await invoke<TaskFetch>('list_project_tasks', { projectId }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setLoadingBoard(false);
    }
  };

  const addFromBoard = (task: BoardTask) => {
    setRows((prev) => {
      // Already on the list: adding it twice would cut two worktrees for one
      // task and then fight over the branch.
      if (prev.some((r) => r.task?.externalId === task.externalId && r.task?.sourceId === task.sourceId)) return prev;
      const blank = prev.findIndex((r) => !r.branch.trim() && !r.prompt.trim());
      const row = { ...rowFromTask(task), accountId: prev.find((r) => r.accountId)?.accountId ?? '' };
      if (blank === -1) return [...prev, row];
      // Fill the empty row rather than appending under it — an empty row left
      // above a filled one looks like something failed.
      return prev.map((r, i) => (i === blank ? row : r));
    });
  };

  const update = (id: string, patch: Partial<TaskRow>) => setRows((prev) => prev.map((r) => (r.id === id ? { ...r, ...patch } : r)));

  // Ask git about each branch as it's typed. Doing this on launch instead would
  // mean discovering that T2 is open in the main clone after three other
  // worktrees already exist.
  useEffect(() => {
    if (!projectId) return;
    const names = rows.map((r) => r.branch.trim()).filter(Boolean);
    let cancelled = false;
    Promise.all(
      names.map((branch) =>
        invoke<BranchAvailability>('check_branch_availability', { projectId, branch }).catch(() => null),
      ),
    ).then((results) => {
      if (cancelled) return;
      const next: Record<string, BranchAvailability> = {};
      for (const r of results) if (r) next[r.branch] = r;
      setAvailability(next);
    });
    return () => {
      cancelled = true;
    };
  }, [projectId, rows.map((r) => r.branch.trim()).join('\u0000')]);

  const rowState = (row: TaskRow) => availability[row.branch.trim()];
  const blocked = rows.some((r) => r.branch.trim() && rowState(r)?.checkedOutAt);
  const ready = rows.filter((r) => r.branch.trim() && r.accountId && !rowState(r)?.checkedOutAt);

  return (
    <div className={styles.picker}>
      <span className={styles.pickerLabel}>SEVERAL TASKS AT ONCE — EACH ON ITS OWN BRANCH</span>
      <div className={styles.fanTop}>
        <select className={styles.multiSelect} value={projectId} onChange={(e) => setProjectId(e.target.value)}>
          <option value="">project…</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        {baseBranch.error && <span className={styles.envWarn}>⚠ {baseBranch.error}</span>}
        {projectId && links.length > 0 && (
          <Button size="sm" variant="outline" disabled={loadingBoard} onClick={loadBoard}>
            {loadingBoard ? 'reading your board…' : `☰ from ${links.map((l) => l.label).join(' + ')}`}
          </Button>
        )}
        {projectId && links.length === 0 && <span className={styles.fanNote}>no task source linked — Settings → Tasks</span>}
      </div>

      {board && (
        <div className={styles.boardList}>
          {board.errors.map((error) => (
            // One source failing doesn't hide the others, so this sits beside
            // the tasks that did arrive instead of replacing them.
            <span key={error} className={styles.envWarn}>
              ⚠ {error}
            </span>
          ))}
          {/* Naming what was asked, because "no tasks" and "asked the wrong
              board" look identical otherwise — and the second one is the one
              you can fix. */}
          {board.tasks.length === 0 && board.errors.length === 0 && (
            <span className={styles.fanNote}>nothing assigned to you in {links.map((l) => l.label).join(' or ')} — narrow it to a list in Settings → tasks if you expected work here</span>
          )}
          {board.tasks.map((task) => {
            const taken = rows.some((r) => r.task?.externalId === task.externalId && r.task?.sourceId === task.sourceId);
            return (
              <button key={`${task.sourceId}-${task.externalId}`} className={taken ? styles.boardTaskTaken : styles.boardTask} onClick={() => addFromBoard(task)} disabled={taken}>
                <span className={styles.boardKey}>{task.key}</span>
                <span className={styles.boardTitle}>{task.title}</span>
                {task.status && <span className={styles.boardStatus}>{task.status}</span>}
                <span className={styles.boardBranch}>⑃ {task.suggestedBranch}</span>
              </button>
            );
          })}
        </div>
      )}

      <div className={styles.multiRows}>
        {rows.map((row) => {
          const state = rowState(row);
          const isNew = Boolean(row.branch.trim()) && state && !state.exists;
          return (
            <div key={row.id} className={styles.taskRow}>
              <span className={styles.fanBranchIcon}>⑃</span>
              <input
                className={state?.checkedOutAt ? styles.fanBranchInputBad : styles.fanBranchInput}
                placeholder="task branch — e.g. T2"
                list={`branches-${row.id}`}
                value={row.branch}
                onChange={(e) => update(row.id, { branch: e.target.value })}
              />
              {/* The project's real branches, as suggestions rather than a
                  closed list: a task whose branch doesn't exist yet is typed
                  here too, and forcing a choice would block that. */}
              <datalist id={`branches-${row.id}`}>
                {(baseBranch.branches ?? []).map((b) => (
                  <option key={b.name} value={b.name} />
                ))}
              </datalist>

              {isNew && (
                <label className={styles.taskBase}>
                  new, from
                  <select className={styles.taskBaseSelect} value={row.baseBranch} onChange={(e) => update(row.id, { baseBranch: e.target.value })}>
                    <option value="">{baseBranch.selected || 'current'}</option>
                    {(baseBranch.branches ?? []).map((b) => (
                      <option key={b.name} value={b.name}>
                        {b.name}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              {state?.checkedOutAt && <span className={styles.taskBlocked}>already checked out at {state.checkedOutAt}</span>}

              <select className={styles.multiSelect} value={row.accountId} onChange={(e) => update(row.id, { accountId: e.target.value })}>
                <option value="">account…</option>
                {accounts.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name} · {PROVIDER_META[a.provider].name}
                  </option>
                ))}
              </select>
              <input
                className={styles.taskPrompt}
                placeholder={row.task ? `brief from ${row.task.key} — type to override` : 'what to tell this agent (optional)'}
                value={row.prompt}
                onChange={(e) => update(row.id, { prompt: e.target.value })}
              />
              <button className={styles.multiRemove} disabled={rows.length === 1} onClick={() => setRows((prev) => (prev.length > 1 ? prev.filter((r) => r.id !== row.id) : prev))}>
                ×
              </button>
            </div>
          );
        })}
      </div>

      {blocked && (
        <span className={styles.envWarn}>
          ⚠ a branch can only be checked out in one place — switch away from it in that worktree, or pick another task
        </span>
      )}

      <div className={styles.multiActions}>
        <button className={styles.addRowLink} onClick={() => setRows((prev) => [...prev, newTaskRow()])}>
          + add task
        </button>
        <span className={styles.spacer} />
        <span className={styles.fanNote}>each task gets its own worktree, on its own branch — you keep all of them</span>
        <Button size="sm" variant="outline" onClick={onClose}>
          cancel
        </Button>
        <Button
          size="sm"
          disabled={!projectId || ready.length === 0 || busy || Boolean(baseBranch.error)}
          onClick={() =>
            onLaunch(
              projectId,
              ready.map((r) => ({
                branch: r.branch.trim(),
                create: !rowState(r)?.exists,
                baseBranch: r.baseBranch || baseBranch.selected || null,
                accountId: r.accountId,
                prompt: r.prompt.trim() || null,
                task: r.task ?? null,
              })),
            )
          }
        >
          {busy ? 'opening…' : `Open ${ready.length || ''}`}
        </Button>
      </div>
    </div>
  );
}

interface FanRow {
  id: string;
  name: string;
  accountId: string;
}

const newFanRow = (n: number): FanRow => ({ id: crypto.randomUUID(), name: `attempt-${n}`, accountId: '' });

/** The branch a row will actually create. Mirrors `slugify` on the Rust side —
 * shown rather than left implicit, because "attempt-1" doesn't look like
 * something that creates a branch, and that's the whole confusion. */
const branchFor = (name: string) => name.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');

/** The fan-out: one prompt, N worktrees, **N accounts**.
 *
 * The account column is the whole point. Fanning a prompt to five agents on one
 * account spends that account's quota five times as fast — the reason people get
 * caught out by this gesture elsewhere. One account per branch means one quota
 * per branch, which is only possible because each account here has its own
 * config dir.
 */
function FanOutPicker({
  projects,
  accounts,
  auth,
  busy,
  onClose,
  onLaunch,
}: {
  projects: Project[];
  accounts: Account[];
  auth: Record<string, AccountAuth>;
  busy: boolean;
  onClose: () => void;
  onLaunch: (projectId: string, branches: FanOutBranch[], prompt: string, baseBranch: string) => void;
}) {
  const [projectId, setProjectId] = useState('');
  const [prompt, setPrompt] = useState('');
  const [rows, setRows] = useState<FanRow[]>([newFanRow(1), newFanRow(2)]);
  const [projection, setProjection] = useState<FanOutProjection[]>([]);
  // Every branch of a fan-out is a worktree, so the same two questions apply
  // before anything can launch: is this a repo, and off which branch?
  const [baseBranch, setBaseBranch] = useBaseBranch(projects.find((p) => p.id === projectId)?.path);

  const update = (id: string, patch: Partial<FanRow>) => setRows((prev) => prev.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  const ready = rows.filter((r) => r.name.trim() && r.accountId);
  const reusedAccount = new Set(ready.map((r) => r.accountId)).size < ready.length;
  // Two rows with the same name mean the same branch, and `git worktree add -b`
  // refuses the second. Caught here rather than after launch, when four of five
  // worktrees already exist.
  const duplicates = new Set(
    rows
      .map((r) => branchFor(r.name))
      .filter((b, i, all) => b && all.indexOf(b) !== i),
  );

  // Pre-flight. A fan-out is the most expensive thing this app can do, and
  // until now it launched blind — you found out what it cost when an account
  // stopped mid-task. The forecast is per account, so the answer is per branch.
  const chosenAccounts = ready.map((r) => r.accountId).join(',');
  useEffect(() => {
    if (!chosenAccounts) {
      setProjection([]);
      return;
    }
    invoke<FanOutProjection[]>('project_fan_out', { accountIds: chosenAccounts.split(',') })
      .then(setProjection)
      .catch(() => setProjection([]));
  }, [chosenAccounts]);

  const tightest = projection.filter((p) => p.hoursLeft != null).sort((a, b) => (a.hoursLeft ?? 0) - (b.hoursLeft ?? 0))[0];
  const benched = projection.filter((p) => p.exhausted);

  return (
    <div className={styles.picker}>
      <span className={styles.pickerLabel}>FAN ONE PROMPT ACROSS SEVERAL ACCOUNTS</span>
      <div className={styles.fanTop}>
        <select className={styles.multiSelect} value={projectId} onChange={(e) => setProjectId(e.target.value)}>
          <option value="">project…</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <BaseBranchPicker projectPath={projects.find((p) => p.id === projectId)?.path} state={baseBranch} onChange={setBaseBranch} />
        <textarea
          className={styles.fanPrompt}
          rows={2}
          placeholder="the prompt every branch gets — e.g. fix the billing rounding and add tests"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
        />
      </div>
      <div className={styles.multiRows}>
        {rows.map((row) => {
          const status = row.accountId ? auth[row.accountId] : undefined;
          return (
            <div key={row.id} className={styles.multiRow}>
              <span className={styles.fanBranchIcon}>⑃</span>
              <input
                className={duplicates.has(branchFor(row.name)) ? styles.fanBranchInputBad : styles.fanBranchInput}
                placeholder="name for this attempt"
                value={row.name}
                onChange={(e) => update(row.id, { name: e.target.value })}
              />
              {/* What the name turns into, and where it comes from. Two
                  attempts can't share a branch — git refuses the same branch in
                  two worktrees — so this is a new branch every time, and saying
                  so beats letting the second one fail after the click. */}
              {branchFor(row.name) && (
                <span className={styles.fanBranchPreview}>
                  creates <code>hangar/{branchFor(row.name)}</code>
                  {baseBranch.selected && <> off <code>{baseBranch.selected}</code></>}
                </span>
              )}
              <span className={styles.multiArrow}>→</span>
              <select className={styles.multiSelect} value={row.accountId} onChange={(e) => update(row.id, { accountId: e.target.value })}>
                <option value="">account…</option>
                {accounts.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name} · {PROVIDER_META[a.provider].name}
                  </option>
                ))}
              </select>
              {status && !status.loggedIn && (
                <span className={styles.statusNeedsLogin} title="will run its login">
                  ✗
                </span>
              )}
              <button
                className={styles.multiRemove}
                title="remove branch"
                disabled={rows.length === 1}
                onClick={() => setRows((prev) => (prev.length > 1 ? prev.filter((r) => r.id !== row.id) : prev))}
              >
                ×
              </button>
            </div>
          );
        })}
      </div>
      {duplicates.size > 0 && (
        <span className={styles.envWarn}>
          ⚠ two attempts would create the same branch — each needs its own name, since a branch can only be checked out in one
          worktree
        </span>
      )}
      {reusedAccount && (
        <span className={styles.envWarn}>
          ⚠ two branches share an account — they'll spend the same quota in parallel, which is exactly what fanning out across accounts avoids
        </span>
      )}
      {benched.length > 0 && (
        <span className={styles.envWarn}>
          ⚠ {benched.map((p) => p.accountName).join(', ')} {benched.length === 1 ? 'is' : 'are'} out of quota — those branches won't start
        </span>
      )}
      {tightest && (
        <span className={styles.fanForecast}>
          ⏱ tightest account: <strong>{tightest.accountName}</strong> has ~{formatHoursLeft(tightest.hoursLeft)} left
          {tightest.limitingWindow === 'weekly' && ' on its weekly window — that one does not come back after lunch'}
        </span>
      )}
      <div className={styles.multiActions}>
        <button className={styles.addRowLink} onClick={() => setRows((prev) => [...prev, newFanRow(prev.length + 1)])}>
          + add branch
        </button>
        <span className={styles.spacer} />
        <span className={styles.fanNote}>each branch gets its own worktree; comparing and merging happens in the Git panel</span>
        <Button size="sm" variant="outline" onClick={onClose}>
          cancel
        </Button>
        <Button
          size="sm"
          disabled={!projectId || !prompt.trim() || ready.length === 0 || busy || !baseBranch.selected || duplicates.size > 0}
          onClick={() => onLaunch(projectId, ready.map(({ name, accountId }) => ({ name, accountId })), prompt, baseBranch.selected)}
        >
          {busy ? 'launching…' : `Fan out ${ready.length || ''}`}
        </Button>
      </div>
    </div>
  );
}

interface MultiRow {
  id: string;
  projectId: string;
  accountId: string;
}

const newMultiRow = (): MultiRow => ({ id: crypto.randomUUID(), projectId: '', accountId: '' });

/** "Multi terminal": a list of (project, account) rows opened all at once —
 * the terminal-first replacement for the old multi-session flow. Deliberately
 * spare next to it: no prompt, model, or mode, because a real interactive
 * Claude Code session in the shell owns all of that itself. */
function MultiTerminalPicker({
  projects,
  accounts,
  auth,
  authLoading,
  onExplore,
  onOpenAll,
  onClose,
}: {
  projects: Project[];
  accounts: Account[];
  auth: Record<string, AccountAuth>;
  authLoading: boolean;
  onExplore: () => Promise<Project | null>;
  onOpenAll: (rows: MultiRow[]) => Promise<void>;
  onClose: () => void;
}) {
  const [rows, setRows] = useState<MultiRow[]>([newMultiRow()]);
  const [busy, setBusy] = useState(false);

  const update = (id: string, patch: Partial<MultiRow>) => setRows((prev) => prev.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  const removeRow = (id: string) => setRows((prev) => (prev.length > 1 ? prev.filter((r) => r.id !== id) : prev));

  const explore = async (rowId: string) => {
    try {
      const project = await onExplore();
      if (project) update(rowId, { projectId: project.id });
    } catch {
      // surfaced by the parent's toast path
    }
  };

  const ready = rows.filter((r) => r.projectId && r.accountId);

  const openAll = async () => {
    if (ready.length === 0 || busy) return;
    setBusy(true);
    try {
      await onOpenAll(ready);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.picker}>
      <span className={styles.pickerLabel}>
        OPEN SEVERAL AT ONCE{authLoading && <span className={styles.pickerChecking}> · checking logins…</span>}
      </span>
      <div className={styles.multiRows}>
        {rows.map((row) => {
          const status = row.accountId && row.accountId !== PLAIN_SHELL ? auth[row.accountId] : undefined;
          return (
            <div key={row.id} className={styles.multiRow}>
              <select className={styles.multiSelect} value={row.projectId} onChange={(e) => (e.target.value === '__explore__' ? explore(row.id) : update(row.id, { projectId: e.target.value }))}>
                <option value="">project…</option>
                {projects.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
                <option value="__explore__">📁 explore a folder…</option>
              </select>
              <span className={styles.multiArrow}>→</span>
              <select className={styles.multiSelect} value={row.accountId} onChange={(e) => update(row.id, { accountId: e.target.value })}>
                <option value="">account…</option>
                {accounts.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name} · {PROVIDER_META[a.provider].name}
                  </option>
                ))}
                <option value={PLAIN_SHELL}>Plain shell</option>
              </select>
              {status && !status.loggedIn && <span className={styles.statusNeedsLogin} title="will run its login">✗</span>}
              <button className={styles.multiRemove} title="remove row" disabled={rows.length === 1} onClick={() => removeRow(row.id)}>
                ×
              </button>
            </div>
          );
        })}
      </div>
      <div className={styles.multiActions}>
        <button className={styles.addRowLink} onClick={() => setRows((prev) => [...prev, newMultiRow()])}>
          + add row
        </button>
        <span className={styles.spacer} />
        <Button size="sm" variant="outline" onClick={onClose}>
          cancel
        </Button>
        <Button size="sm" disabled={ready.length === 0 || busy} onClick={openAll}>
          {busy ? 'opening…' : `Open ${ready.length || ''}`}
        </Button>
      </div>
    </div>
  );
}
