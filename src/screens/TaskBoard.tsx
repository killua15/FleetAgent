import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { toastError } from '../components/ErrorOverlay';
import { Button } from '../components/ui/button';
import { useNavigation } from '../nav/NavigationProvider';
import { PROVIDER_META, type Account } from '../types/account';
import type { Project } from '../types/session';
import type { BoardTask, TaskFetch, TaskSourceLink } from '../types/task';
import type { TerminalInfo } from '../types/terminal';
import styles from './TaskBoard.module.css';

/**
 * Your board, with a terminal at the end of every row.
 *
 * The ☰ Tasks picker inside Terminals is for the moment you already know what
 * you're doing and want four worktrees at once. This is the other moment:
 * you don't know yet, and you want to see what's assigned to you before
 * deciding. Same fetch, same `open_task_worktrees` underneath — the difference
 * is that here the list is the screen rather than a step inside a launcher.
 */
export function TaskBoard() {
  const { navigate } = useNavigation();
  const [projects, setProjects] = useState<Project[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [projectId, setProjectId] = useState('');
  const [links, setLinks] = useState<TaskSourceLink[]>([]);
  const [board, setBoard] = useState<TaskFetch | null>(null);
  const [loading, setLoading] = useState(false);
  const [sourceFilter, setSourceFilter] = useState<string>('all');
  const [picked, setPicked] = useState<Record<string, BoardTask>>({});
  const [accountId, setAccountId] = useState('');
  const [opening, setOpening] = useState(false);

  useEffect(() => {
    Promise.all([
      invoke<Project[]>('list_projects').then((found) => {
        setProjects(found);
        // One project is the common case and choosing it for the user removes
        // a click that has only one right answer.
        if (found.length === 1) setProjectId(found[0].id);
      }),
      invoke<Account[]>('list_accounts').then(setAccounts),
    ]).catch((e) => toastError(String(e)));
  }, []);

  useEffect(() => {
    setBoard(null);
    setPicked({});
    if (!projectId) {
      setLinks([]);
      return;
    }
    invoke<TaskSourceLink[]>('list_task_source_links', { projectId })
      .then((found) => {
        setLinks(found);
        if (found.length > 0) load(projectId);
      })
      .catch(() => setLinks([]));
  }, [projectId]);

  const load = async (id: string) => {
    setLoading(true);
    try {
      setBoard(await invoke<TaskFetch>('list_project_tasks', { projectId: id }));
    } catch (e) {
      toastError(String(e));
      setBoard({ tasks: [], errors: [String(e)] });
    } finally {
      setLoading(false);
    }
  };

  const idOf = (task: BoardTask) => `${task.sourceId}:${task.externalId}`;

  const toggle = (task: BoardTask) =>
    setPicked((prev) => {
      const next = { ...prev };
      if (next[idOf(task)]) delete next[idOf(task)];
      else next[idOf(task)] = task;
      return next;
    });

  const chosen = Object.values(picked);
  const shown = (board?.tasks ?? []).filter((t) => sourceFilter === 'all' || t.sourceId === sourceFilter);

  const send = async () => {
    if (!accountId || chosen.length === 0) return;
    setOpening(true);
    try {
      await invoke<TerminalInfo[]>('open_task_worktrees', {
        projectId,
        tasks: chosen.map((task) => ({
          branch: task.suggestedBranch,
          // Every board task is new work by definition — a branch that already
          // exists is caught by the backend, which attaches to it instead.
          create: true,
          baseBranch: null,
          accountId,
          prompt: null,
          task,
        })),
        cols: 80,
        rows: 24,
      });
      navigate('terminals');
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };

  return (
    <div className={styles.screen}>
      <AppTopBar />

      <div className={styles.head}>
        <button className={styles.backLink} onClick={() => navigate('terminals')}>
          ← back
        </button>
        <span className={styles.title}>Tasks</span>
        <span className={styles.subtitle}>what's assigned to you — pick some and each one gets a worktree on its own branch</span>
        <span className={styles.spacer} />
        <select className={styles.select} value={projectId} onChange={(e) => setProjectId(e.target.value)}>
          <option value="">project…</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <Button size="sm" variant="outline" disabled={!projectId || loading} onClick={() => load(projectId)}>
          {loading ? 'reading…' : '↻ refresh'}
        </Button>
      </div>

      {projectId && links.length > 1 && (
        <div className={styles.tabs}>
          <button className={sourceFilter === 'all' ? styles.tabActive : styles.tab} onClick={() => setSourceFilter('all')}>
            all {board ? board.tasks.length : ''}
          </button>
          {links.map((link) => (
            <button key={link.sourceId} className={sourceFilter === link.sourceId ? styles.tabActive : styles.tab} onClick={() => setSourceFilter(link.sourceId)}>
              {link.label} {board ? board.tasks.filter((t) => t.sourceId === link.sourceId).length : ''}
            </button>
          ))}
        </div>
      )}

      <div className={styles.list}>
        {!projectId && <span className={styles.empty}>Pick a project to see its board.</span>}
        {projectId && links.length === 0 && (
          <span className={styles.empty}>
            This project isn't wired to a board yet.{' '}
            <button className={styles.inlineLink} onClick={() => navigate('task-settings')}>
              Settings → tasks
            </button>
          </span>
        )}
        {board?.errors.map((error) => (
          // Beside the tasks, never instead of them: one dead source must not
          // hide the other's work.
          <span key={error} className={styles.error}>
            ⚠ {error}
          </span>
        ))}
        {board && shown.length === 0 && board.errors.length === 0 && !loading && (
          <span className={styles.empty}>Nothing assigned to you in {links.map((l) => l.label).join(' or ')}.</span>
        )}

        {shown.map((task) => {
          const isPicked = Boolean(picked[idOf(task)]);
          return (
            <div key={idOf(task)} className={isPicked ? styles.rowPicked : styles.row} onClick={() => toggle(task)}>
              <input type="checkbox" checked={isPicked} onChange={() => toggle(task)} onClick={(e) => e.stopPropagation()} />
              <span className={styles.key}>{task.key}</span>
              <span className={styles.taskTitle}>{task.title}</span>
              {task.container && <span className={styles.container}>{task.container}</span>}
              {task.status && <span className={styles.status}>{task.status}</span>}
              <span className={styles.branch}>⑃ {task.suggestedBranch}</span>
              {/* Opens in the browser rather than in Hangar: reading the full
                  ticket means comments and attachments, which is the board's
                  job, not ours. */}
              <a className={styles.openLink} href={task.url} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()}>
                ↗
              </a>
            </div>
          );
        })}
      </div>

      {chosen.length > 0 && (
        <div className={styles.bar}>
          <span className={styles.barCount}>
            {chosen.length} task{chosen.length === 1 ? '' : 's'} — {chosen.length === 1 ? 'one worktree' : `${chosen.length} worktrees, one branch each`}
          </span>
          <span className={styles.spacer} />
          <select className={styles.select} value={accountId} onChange={(e) => setAccountId(e.target.value)}>
            <option value="">account…</option>
            {accounts.map((a) => (
              <option key={a.id} value={a.id}>
                {a.name} · {PROVIDER_META[a.provider].name}
              </option>
            ))}
          </select>
          <Button size="sm" disabled={!accountId || opening} onClick={send}>
            {opening ? 'opening…' : 'Send to terminals'}
          </Button>
        </div>
      )}
    </div>
  );
}
