import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { toastError } from '../components/ErrorOverlay';
import { Button } from '../components/ui/button';
import { useNavigation } from '../nav/NavigationProvider';
import { providerMeta, type Account } from '../types/account';
import type { Project } from '../types/session';
import { TASK_SOURCE_META, type BoardTask, type TaskFetch, type TaskSource } from '../types/task';
import type { TerminalInfo } from '../types/terminal';
import { bySource as tasksBySource, containerOptions, filterTasks, statusOptions } from '../lib/taskFilters';
import styles from './TaskBoard.module.css';

/**
 * Your board, with a terminal at the end of every row.
 *
 * The only way into a task now: pick a board, see what's assigned to you, and
 * turn the ones you want into worktrees. It replaced a launcher panel inside
 * Terminals that asked you to type branch names before you'd looked at
 * anything — the list has to come first, because "what am I picking up" is the
 * question you answer by reading, not by remembering.
 */
export function TaskBoard() {
  const { navigate } = useNavigation();
  const [projects, setProjects] = useState<Project[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [projectId, setProjectId] = useState('');
  // Every source configured in Settings, not only the ones this project narrows
  // down. They're the tabs: one account per board — Technomar's ClickUp, the
  // other org's GitHub — and each keeps its own credential.
  const [sources, setSources] = useState<TaskSource[]>([]);
  const [board, setBoard] = useState<TaskFetch | null>(null);
  const [loading, setLoading] = useState(false);
  const [sourceFilter, setSourceFilter] = useState<string>('all');
  // The board's own project — a ClickUp list, a GitHub repo. Distinct from the
  // Hangar project below, which decides *where the worktree is cut*: one repo
  // can take work from three lists, and one list can feed two repos.
  const [boardFilter, setBoardFilter] = useState<string>('all');
  const [statusFilter, setStatusFilter] = useState<string>('all');
  const [picked, setPicked] = useState<Record<string, BoardTask>>({});
  const [accountId, setAccountId] = useState('');
  // What you'd otherwise type into every terminal once it's up: "plan only",
  // "don't touch the migrations". Free text, and one for the whole selection —
  // like the project and the account it sits next to.
  const [notes, setNotes] = useState('');
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
      invoke<TaskSource[]>('list_task_sources').then((found) => {
        setSources(found);
        // One board is the common case; making you choose it from a list of one
        // is a step with a single right answer.
        if (found.length === 1) setSourceFilter(found[0].id);
      }),
    ]).catch((e) => toastError(String(e)));
  }, []);

  // The account decides what's on screen; the project only decides where a
  // worktree lands. Reading the board no longer waits for a project to be
  // chosen — which was backwards, since "which board am I working from" is the
  // question you can answer without looking anything up.
  useEffect(() => {
    setBoard(null);
    setPicked({});
    setBoardFilter('all');
    setStatusFilter('all');
    if (sourceFilter !== 'all') load(sourceFilter);
  }, [sourceFilter]);

  const load = async (sourceId: string) => {
    setLoading(true);
    try {
      setBoard(await invoke<TaskFetch>('list_source_tasks', { sourceId }));
    } catch (e) {
      // Shown in the list, not as a toast: four stacked toasts saying the same
      // thing is what a failed fetch looked like before, and none of them said
      // which source it was.
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
  const all = board?.tasks ?? [];

  // Options come from what actually arrived, not from a fixed list: ClickUp
  // statuses are per-space and made up by whoever set the board up
  // ("verificar", "pronto para teste"), so anything hardcoded would be wrong
  // for everyone but the person who wrote it.
  // In `lib/taskFilters` rather than here: this is the screen's only real
  // decision, and it has edge cases — a task with no list, a source with
  // nothing, options that must come from the data because ClickUp statuses are
  // invented per space — that deserve tests more than a render does.
  const filters = { sourceId: sourceFilter, container: boardFilter, status: statusFilter };
  const bySource = tasksBySource(all, sourceFilter);
  const boards = containerOptions(all, sourceFilter);
  const statuses = statusOptions(all, sourceFilter);
  const shown = filterTasks(all, filters);

  const send = async () => {
    if (!accountId || !projectId || chosen.length === 0) return;
    setOpening(true);
    try {
      const opened = await invoke<TerminalInfo[]>('open_task_worktrees', {
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
          notes: notes.trim() || null,
        })),
        cols: 80,
        rows: 24,
      });
      // Restores whichever panel was minimized. Several tasks can open at
      // once (multi-select); focusing the first is still better than
      // landing on a screen where none of them are visibly open.
      navigate('terminals', { focusTerminalId: opened[0]?.id });
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
        <span className={styles.fieldLabel}>account</span>
        <select className={styles.select} value={sourceFilter} onChange={(e) => setSourceFilter(e.target.value)}>
          <option value="all">choose a board…</option>
          {sources.map((source) => (
            <option key={source.id} value={source.id}>
              {source.label} · {TASK_SOURCE_META[source.kind]?.name ?? source.kind}
            </option>
          ))}
        </select>
        <Button size="sm" variant="outline" disabled={sourceFilter === 'all' || loading} onClick={() => load(sourceFilter)}>
          {loading ? 'reading…' : '↻ refresh'}
        </Button>
      </div>

      {board && board.tasks.length > 0 && (
        <div className={styles.filters}>
          <label className={styles.filter}>
            project
            <select className={styles.select} value={boardFilter} onChange={(e) => setBoardFilter(e.target.value)}>
              <option value="all">all</option>
              {boards.map((name) => (
                <option key={name} value={name}>
                  {name} ({bySource.filter((t) => t.container === name).length})
                </option>
              ))}
            </select>
          </label>

          <label className={styles.filter}>
            status
            <select className={styles.select} value={statusFilter} onChange={(e) => setStatusFilter(e.target.value)}>
              <option value="all">all</option>
              {statuses.map((name) => (
                <option key={name} value={name}>
                  {name} ({bySource.filter((t) => t.status === name).length})
                </option>
              ))}
            </select>
          </label>

          <span className={styles.spacer} />
          <span className={styles.count}>
            {shown.length === all.length ? `${all.length} tasks` : `${shown.length} of ${all.length}`}
          </span>
          {(boardFilter !== 'all' || statusFilter !== 'all') && (
            <button className={styles.inlineLink} onClick={() => { setBoardFilter('all'); setStatusFilter('all'); }}>
              clear
            </button>
          )}
        </div>
      )}

      <div className={styles.list}>
        {sources.length > 0 && sourceFilter === 'all' && <span className={styles.empty}>Choose a board above to see what's assigned to you.</span>}
        {sources.length === 0 && (
          <span className={styles.empty}>
            No board configured yet.{' '}
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
          <span className={styles.empty}>
            {all.length === 0
              ? `Nothing assigned to you in ${sources.find((s) => s.id === sourceFilter)?.label ?? 'this board'}.`
              : `No task matches these filters — ${all.length} are hidden by them.`}
          </span>
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
          <label className={styles.notesField}>
            <span className={styles.fieldLabel}>
              notes for the agent{chosen.length > 1 ? ` — applies to all ${chosen.length} tasks` : ''}
            </span>
            {/* Goes into each task's brief above the ticket, so it can be as
                long and as many lines as it needs. */}
            <textarea
              className={styles.notes}
              rows={2}
              value={notes}
              onChange={(e) => setNotes(e.target.value)}
              placeholder="optional — e.g. plan only, don't touch the migrations, open a PR when it's green"
            />
          </label>
          <div className={styles.barRow}>
            <span className={styles.barCount}>
              {chosen.length} task{chosen.length === 1 ? '' : 's'} — {chosen.length === 1 ? 'one worktree' : `${chosen.length} worktrees, one branch each`}
            </span>
            <span className={styles.spacer} />
            <span className={styles.fieldLabel}>worktrees in</span>
            <select className={styles.select} value={projectId} onChange={(e) => setProjectId(e.target.value)}>
              <option value="">project…</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
            <select className={styles.select} value={accountId} onChange={(e) => setAccountId(e.target.value)}>
              <option value="">account…</option>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name} · {providerMeta(a.provider).name}
                </option>
              ))}
            </select>
            <Button size="sm" disabled={!accountId || !projectId || opening} onClick={send}>
              {opening ? 'opening…' : 'Send to terminals'}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
