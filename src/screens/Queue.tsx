import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { Button } from '../components/ui/button';
import { Spinner } from '../components/Spinner';
import { toastError } from '../components/ErrorOverlay';
import { formatHoursLeft, type Account, type AccountQuota } from '../types/account';
import type { Project } from '../types/session';
import styles from './Queue.module.css';

interface QueuedRun {
  id: string;
  projectId: string;
  projectName: string;
  prompt: string;
  branchName: string;
  accountId: string | null;
  accountName: string | null;
  state: 'queued' | 'running' | 'done' | 'failed' | 'cancelled';
  terminalId: string | null;
  attempts: number;
  lastError: string | null;
  createdAt: string;
  startedAt: string | null;
  finishedAt: string | null;
}

interface ProjectBudget {
  projectId: string;
  projectName: string;
  weeklyPct: number | null;
  spentPct: number;
  estimated: boolean;
}

const STATE_LABEL: Record<QueuedRun['state'], string> = {
  queued: 'waiting for capacity',
  running: 'running',
  done: 'agent stopped',
  failed: 'could not start',
  cancelled: 'cancelled',
};

/**
 * Work that waits for quota instead of failing against it.
 *
 * The wording here is deliberate in two places. A finished run says "agent
 * stopped", not "done" — the queue knows the agent stopped producing output and
 * has no idea whether the work is any good, so claiming a verdict it can't
 * reach would be the one thing that makes people stop trusting the screen. And
 * queued runs say what they're waiting *for*, because "queued" alone is the
 * least useful word a queue can show you.
 */
export function Queue() {
  const [runs, setRuns] = useState<QueuedRun[]>([]);
  const [quotas, setQuotas] = useState<AccountQuota[]>([]);
  const [budgets, setBudgets] = useState<ProjectBudget[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [loading, setLoading] = useState(true);

  const [projectId, setProjectId] = useState('');
  const [prompt, setPrompt] = useState('');
  const [branch, setBranch] = useState('');
  const [accountId, setAccountId] = useState('');
  const [queueing, setQueueing] = useState(false);

  const load = () =>
    Promise.all([
      invoke<QueuedRun[]>('list_queued_runs').then(setRuns),
      invoke<AccountQuota[]>('list_account_quotas').then(setQuotas),
      invoke<ProjectBudget[]>('list_project_budgets').then(setBudgets),
    ]).catch((e) => toastError(String(e)));

  useEffect(() => {
    Promise.all([load(), invoke<Project[]>('list_projects').then(setProjects), invoke<Account[]>('list_accounts').then(setAccounts)]).finally(() =>
      setLoading(false),
    );
    const unlisten = listen('queue-updated', () => {
      load();
    });
    return () => {
      unlisten.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const enqueue = async () => {
    if (!projectId || !prompt.trim() || !branch.trim()) return;
    setQueueing(true);
    try {
      await invoke('enqueue_run', { projectId, prompt, branchName: branch, accountId: accountId || null });
      setPrompt('');
      setBranch('');
      await load();
    } catch (e) {
      toastError(String(e));
    } finally {
      setQueueing(false);
    }
  };

  const cancel = async (id: string) => {
    try {
      await invoke('cancel_queued_run', { id });
      await load();
    } catch (e) {
      toastError(String(e));
    }
  };

  const setBudget = async (project: string, pct: number | null) => {
    try {
      await invoke('set_project_budget', { projectId: project, weeklyPct: pct });
      await load();
    } catch (e) {
      toastError(String(e));
    }
  };

  // The soonest any account frees up — what a queued run is actually waiting
  // for. `null` when something already has room, in which case the next poll
  // pass starts it.
  const soonest = quotas
    .map((q) => q.session?.hoursLeft)
    .filter((h): h is number => h != null)
    .sort((a, b) => a - b)[0];
  const anyFree = quotas.some((q) => (q.session?.pct ?? 0) < 70 && q.status === 'healthy');

  const active = runs.filter((r) => r.state === 'queued' || r.state === 'running');
  const past = runs.filter((r) => r.state === 'done' || r.state === 'failed');

  return (
    <div className={styles.screen}>
      <AppTopBar />
      <div className={styles.body}>
        <section className={styles.compose}>
          <span className={styles.sectionLabel}>QUEUE WORK</span>
          <div className={styles.composeRow}>
            <select className={styles.select} value={projectId} onChange={(e) => setProjectId(e.target.value)}>
              <option value="">project…</option>
              {projects.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                </option>
              ))}
            </select>
            <input className={styles.input} placeholder="branch name" value={branch} onChange={(e) => setBranch(e.target.value)} />
            <select className={styles.select} value={accountId} onChange={(e) => setAccountId(e.target.value)}>
              <option value="">any allowed account</option>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
          </div>
          <textarea className={styles.prompt} rows={3} placeholder="what should the agent do?" value={prompt} onChange={(e) => setPrompt(e.target.value)} />
          <div className={styles.composeActions}>
            <span className={styles.hint}>
              Runs start on their own when an account has room, each in its own worktree. Hangar has to stay open — this waits for
              quota, not for your laptop.
            </span>
            <Button size="sm" disabled={!projectId || !prompt.trim() || !branch.trim() || queueing} onClick={enqueue}>
              {queueing ? 'queueing…' : 'Queue it'}
            </Button>
          </div>
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>
            IN THE QUEUE
            {!anyFree && soonest != null && <span className={styles.waitHint}> · every account is busy; soonest frees in ~{formatHoursLeft(soonest)}</span>}
          </span>
          {loading && <div className={styles.dim}><Spinner label="loading…" /></div>}
          {!loading && active.length === 0 && <span className={styles.dim}>Nothing queued.</span>}
          {active.map((run) => (
            <div key={run.id} className={styles.run}>
              <span className={run.state === 'running' ? styles.dotRunning : styles.dotQueued} />
              <div className={styles.runMain}>
                <div className={styles.runTop}>
                  <span className={styles.runProject}>{run.projectName}</span>
                  <span className={styles.runBranch}>⑃ {run.branchName}</span>
                  {run.accountName && <span className={styles.runAccount}>{run.accountName}</span>}
                  <span className={styles.runState}>{STATE_LABEL[run.state]}</span>
                </div>
                <span className={styles.runPrompt}>{run.prompt}</span>
                {run.attempts > 0 && run.lastError && <span className={styles.runError}>attempt {run.attempts}: {run.lastError}</span>}
              </div>
              {run.state === 'queued' && (
                <Button variant="outline" size="sm" onClick={() => cancel(run.id)}>
                  cancel
                </Button>
              )}
            </div>
          ))}
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>WEEKLY BUDGETS</span>
          <span className={styles.hint}>
            A cap on how much of the weekly pool a project may spend. It blocks new runs only — never one already in flight, because
            stopping an agent mid-edit leaves a worktree nobody asked for.
          </span>
          {budgets.map((b) => (
            <div key={b.projectId} className={styles.budget}>
              <span className={styles.budgetName}>{b.projectName}</span>
              <div className={styles.budgetBar}>
                <div
                  className={b.weeklyPct != null && b.spentPct >= b.weeklyPct ? styles.budgetFillOver : styles.budgetFill}
                  style={{ width: `${Math.min(100, b.weeklyPct ? (b.spentPct / b.weeklyPct) * 100 : b.spentPct)}%` }}
                />
              </div>
              <span className={styles.budgetSpent} title={b.estimated ? 'Estimated: this project shared an account with another, and usage is only measured per account' : undefined}>
                {b.spentPct.toFixed(1)}%{b.estimated && ' ≈'}
              </span>
              <input
                className={styles.budgetInput}
                type="number"
                min={1}
                max={100}
                placeholder="no cap"
                value={b.weeklyPct ?? ''}
                onChange={(e) => setBudget(b.projectId, e.target.value === '' ? null : Number(e.target.value))}
              />
            </div>
          ))}
          {budgets.some((b) => b.estimated) && (
            <span className={styles.hint}>
              ≈ marks an estimate. Usage is sampled per account, so when two projects share one the split between them is a guess.
              It becomes exact with team leases, which record who held which account and when.
            </span>
          )}
        </section>

        {past.length > 0 && (
          <section className={styles.section}>
            <span className={styles.sectionLabel}>RECENT</span>
            {past.slice(0, 8).map((run) => (
              <div key={run.id} className={styles.runPast}>
                <span className={styles.runProject}>{run.projectName}</span>
                <span className={styles.runBranch}>⑃ {run.branchName}</span>
                <span className={styles.runState}>{STATE_LABEL[run.state]}</span>
                {run.lastError && <span className={styles.runError}>{run.lastError}</span>}
              </div>
            ))}
            <span className={styles.hint}>
              “Agent stopped” means it stopped producing output, not that the work is good — open its worktree to see what happened.
            </span>
          </section>
        )}
      </div>
    </div>
  );
}
