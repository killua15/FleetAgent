import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { ago } from '../lib/notifications';
import { orderedStatuses, terminalForTask } from '../lib/tasksPanel';
import { useNavigation } from '../nav/NavigationProvider';
import { providerMeta, type Account } from '../types/account';
import type { Project } from '../types/session';
import { TASK_SOURCE_META, type BoardTask, type CachedSource, type TaskSourceKind } from '../types/task';
import type { TerminalInfo } from '../types/terminal';
import { toastError } from './ErrorOverlay';
import { Button } from './ui/button';
import styles from './TasksSidebar.module.css';

const ALL = 'all';

/** Per-viewer conveniences — storage may be unavailable, and the defaults work. */
function remembered(key: string): string {
  try {
    return localStorage.getItem(`hangar.tasksPanel.${key}`) ?? '';
  } catch {
    return '';
  }
}
function remember(key: string, value: string) {
  try {
    localStorage.setItem(`hangar.tasksPanel.${key}`, value);
  } catch {
    // Not remembered; still applied.
  }
}

/**
 * Your boards next to the terminals: what's assigned to you on each provider,
 * filtered by status, and which of those tasks already has a terminal.
 *
 * Reads the backend's cache (refreshed every 5 minutes by the task poll), so
 * opening it costs nothing and it stays current on its own. The Tasks screen
 * is still where several tasks get picked and sent at once; this is for
 * keeping an eye on the board while you work, and picking up one at a time.
 */
export function TasksSidebar({
  terminals,
  projects,
  accounts,
  focused,
  onFocusTerminal,
  onOpened,
}: {
  terminals: TerminalInfo[];
  projects: Project[];
  accounts: Account[];
  focused: TerminalInfo | null;
  onFocusTerminal: (id: string) => void;
  onOpened: (opened: TerminalInfo[]) => void;
}) {
  const { navigate } = useNavigation();
  const [sources, setSources] = useState<CachedSource[] | null>(null);
  const [configured, setConfigured] = useState<number | null>(null);
  const [sourceId, setSourceId] = useState(() => remembered('source'));
  const [status, setStatus] = useState(ALL);
  const [branches, setBranches] = useState<Record<string, string>>({});
  const [refreshing, setRefreshing] = useState(false);
  const [openingId, setOpeningId] = useState<string | null>(null);
  const [projectId, setProjectId] = useState(() => remembered('project'));
  const [accountId, setAccountId] = useState(() => remembered('account'));
  // Re-render once a minute so "updated 3m ago" doesn't freeze.
  const [, setTick] = useState(0);

  const refresh = async (only: string | null) => {
    setRefreshing(true);
    try {
      setSources(await invoke<CachedSource[]>('refresh_tasks', { sourceId: only }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setRefreshing(false);
    }
  };

  useEffect(() => {
    invoke<{ id: string }[]>('list_task_sources')
      .then((list) => setConfigured(list.length))
      .catch(() => setConfigured(0));
    invoke<CachedSource[]>('list_cached_tasks')
      .then((cached) => {
        setSources(cached);
        // Opened before the poll's first read: read now rather than sit empty.
        if (cached.length === 0 || cached.some((s) => !s.fetchedAt && !s.error)) refresh(null);
      })
      .catch(() => setSources([]));
    const unlisten = listen('tasks-updated', () => {
      invoke<CachedSource[]>('list_cached_tasks').then(setSources).catch(() => {});
    });
    const timer = window.setInterval(() => setTick((n) => n + 1), 60_000);
    return () => {
      unlisten.then((f) => f());
      window.clearInterval(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Which branch each terminal is on — one `git` per terminal, so asked when
  // the set of terminals changes and every couple of minutes, not on every
  // agent stop: with twenty panels that was a burst of processes per turn.
  const terminalKey = terminals.map((t) => t.id).join(',');
  useEffect(() => {
    const load = () => invoke<Record<string, string>>('terminal_branches').then(setBranches).catch(() => setBranches({}));
    load();
    const timer = window.setInterval(load, 120_000);
    return () => window.clearInterval(timer);
  }, [terminalKey]);

  // Defaults for where "open in a terminal" lands: what you picked last, else
  // the focused terminal's project and account, else the only one there is.
  useEffect(() => {
    if (!projectId || !projects.some((p) => p.id === projectId)) {
      const fromFocus = focused?.kind === 'project' ? focused.sourceId : '';
      const visible = projects.filter((p) => !p.hidden);
      setProjectId(fromFocus || (visible.length === 1 ? visible[0].id : ''));
    }
    if (!accountId || !accounts.some((a) => a.id === accountId)) {
      setAccountId(focused?.accountId ?? (accounts.length === 1 ? accounts[0].id : ''));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projects, accounts, focused?.id]);

  const list = sources ?? [];
  const current = list.find((s) => s.sourceId === sourceId) ?? list[0] ?? null;
  const tasks = current?.tasks ?? [];
  const statuses = orderedStatuses(tasks);
  const shown = status === ALL ? tasks : tasks.filter((t) => t.status === status);

  const pickSource = (id: string) => {
    setSourceId(id);
    setStatus(ALL);
    remember('source', id);
  };

  const open = async (task: BoardTask) => {
    if (!projectId || !accountId) {
      toastError('Choose the project and the account below first.');
      return;
    }
    setOpeningId(task.externalId);
    try {
      const opened = await invoke<TerminalInfo[]>('open_task_worktrees', {
        projectId,
        tasks: [{ branch: task.suggestedBranch, create: true, baseBranch: null, accountId, prompt: null, task, notes: null }],
        cols: 80,
        rows: 24,
      });
      onOpened(opened);
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpeningId(null);
    }
  };

  if (configured === 0) {
    return (
      <aside className={styles.sidebar}>
        <span className={styles.empty}>
          No board configured yet.{' '}
          <button className={styles.link} onClick={() => navigate('task-settings')}>
            Settings → tasks
          </button>
        </span>
      </aside>
    );
  }

  return (
    <aside className={styles.sidebar}>
      <div className={styles.head}>
        <span className={styles.title}>Tasks</span>
        {current?.fetchedAt && <span className={styles.dim}>updated {ago(current.fetchedAt)}</span>}
        <span className={styles.spacer} />
        <button className={styles.link} onClick={() => navigate('task-board')} title="The full board: pick several tasks and send them at once">
          board ↗
        </button>
        <Button size="sm" variant="ghost" disabled={refreshing} onClick={() => refresh(current?.sourceId ?? null)} title="Read this board again now">
          {refreshing ? '…' : '↻'}
        </Button>
      </div>

      {list.length > 1 && (
        <div className={styles.tabs}>
          {list.map((s) => (
            <button
              key={s.sourceId}
              className={s.sourceId === current?.sourceId ? styles.tabOn : styles.tab}
              onClick={() => pickSource(s.sourceId)}
              title={TASK_SOURCE_META[s.kind as TaskSourceKind]?.name ?? s.kind}
            >
              {s.label} <span className={styles.count}>{s.tasks.length}</span>
            </button>
          ))}
        </div>
      )}

      {tasks.length > 0 && (
        <div className={styles.chips}>
          <button className={status === ALL ? styles.chipOn : styles.chip} onClick={() => setStatus(ALL)}>
            all <span className={styles.count}>{tasks.length}</span>
          </button>
          {statuses.map(({ status: name, count }) => (
            <button key={name} className={status === name ? styles.chipOn : styles.chip} onClick={() => setStatus(name)}>
              {name} <span className={styles.count}>{count}</span>
            </button>
          ))}
        </div>
      )}

      <div className={styles.body}>
        {current?.error && <span className={styles.error}>⚠ {current.error}</span>}
        {!sources && <span className={styles.empty}>Reading your boards…</span>}
        {sources && !current && <span className={styles.empty}>{refreshing ? 'Reading your boards…' : 'Nothing read yet — press ↻.'}</span>}
        {current && current.fetchedAt && tasks.length === 0 && <span className={styles.empty}>Nothing assigned to you in {current.label}.</span>}

        {shown.map((task) => {
          const terminalId = terminalForTask(task, branches);
          const terminal = terminalId ? terminals.find((t) => t.id === terminalId) : null;
          return (
            <div key={`${task.sourceId}:${task.externalId}`} className={styles.row}>
              <div className={styles.rowTop}>
                <span className={styles.key}>{task.key}</span>
                {task.status && <span className={styles.status}>{task.status}</span>}
                <span className={styles.spacer} />
                <a className={styles.ext} href={task.url} target="_blank" rel="noreferrer" title="Open on the board">
                  ↗
                </a>
              </div>
              <span className={styles.taskTitle} title={task.title}>
                {task.title}
              </span>
              <div className={styles.rowBottom}>
                {task.container && <span className={styles.dim}>{task.container}</span>}
                <span className={styles.spacer} />
                {terminal ? (
                  <button className={styles.inTerminal} onClick={() => onFocusTerminal(terminal.id)} title={`Go to ${terminal.displayName ?? terminal.title}`}>
                    ⑃ in terminal
                  </button>
                ) : (
                  <button
                    className={styles.openBtn}
                    disabled={openingId !== null}
                    onClick={() => open(task)}
                    title={`New worktree on ${task.suggestedBranch}, with a terminal on it`}
                  >
                    {openingId === task.externalId ? 'opening…' : '▶ open'}
                  </button>
                )}
              </div>
            </div>
          );
        })}
      </div>

      <div className={styles.foot}>
        <span className={styles.dim}>open in</span>
        <select
          className={styles.select}
          value={projectId}
          onChange={(e) => {
            setProjectId(e.target.value);
            remember('project', e.target.value);
          }}
        >
          <option value="">project…</option>
          {projects
            .filter((p) => !p.hidden)
            .map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
        </select>
        <select
          className={styles.select}
          value={accountId}
          onChange={(e) => {
            setAccountId(e.target.value);
            remember('account', e.target.value);
          }}
        >
          <option value="">account…</option>
          {accounts.map((a) => (
            <option key={a.id} value={a.id}>
              {a.name} · {providerMeta(a.provider).name}
            </option>
          ))}
        </select>
      </div>
    </aside>
  );
}
