import type { TaskWorktree } from '../types/gitPanel';
import type { BoardTask } from '../types/task';

/** Where each kind of status sits in the chip row: work still to start, work
 * in flight, then work that's finished. ClickUp says which kind each of its
 * invented statuses is; anything it doesn't classify goes with the middle. */
const KIND_RANK: Record<string, number> = { open: 0, custom: 1, done: 2, closed: 3 };

/** The statuses present in `tasks`, each with its count, in board order. */
export function orderedStatuses(tasks: BoardTask[]): { status: string; count: number }[] {
  const seen = new Map<string, { count: number; rank: number }>();
  for (const task of tasks) {
    if (!task.status) continue;
    const entry = seen.get(task.status) ?? { count: 0, rank: KIND_RANK[task.statusKind ?? ''] ?? 1 };
    entry.count += 1;
    seen.set(task.status, entry);
  }
  return [...seen.entries()]
    .sort(([a, x], [b, y]) => x.rank - y.rank || a.localeCompare(b))
    .map(([status, { count }]) => ({ status, count }));
}

/** How well a branch or folder name names `task`: 2 is its own branch, 1 a
 * name that mentions its key, 0 nothing.
 *
 * Distinctive keys (`DEV-42`, `CU-86akmv`) match a segment equal to them or
 * starting with `key-`. A bare issue number only matches in the shapes people
 * actually give it — `128`, `128-fix`, `issue-128-…`, `gh-128`, `#128` — never
 * as digits inside something else (`release-128`, `v1.12`). `numbers: false`
 * turns the number rule off, for names that could come from any repo. */
export function matchRank(task: BoardTask, name: string, { numbers = true } = {}): number {
  const lower = name.toLowerCase();
  if ([task.suggestedBranch, task.branchHint].some((b) => b && b.toLowerCase() === lower)) return 2;
  const key = task.key.toLowerCase();
  if (/^\d+$/.test(key)) {
    if (!numbers) return 0;
    if (lower.includes(`#${key}`)) return 1;
    const shape = new RegExp(`^(issues?-|gh-)?${key}(-|$)`);
    return lower.split('/').some((segment) => shape.test(segment)) ? 1 : 0;
  }
  return lower.split(/[/_\s]/).some((part) => part === key || part.startsWith(`${key}-`)) ? 1 : 0;
}

/** The worktree already cut for `task`, by its branch or its folder name. A
 * GitHub issue only looks in worktrees of its own repo: `#12` is a different
 * issue in every one. */
export function worktreeForTask(task: BoardTask, worktrees: TaskWorktree[]): TaskWorktree | null {
  const repo = task.sourceKind === 'github' ? task.container?.toLowerCase() : null;
  let best: TaskWorktree | null = null;
  let bestRank = 0;
  for (const wt of worktrees) {
    if (repo && wt.repo?.toLowerCase() !== repo) continue;
    const rank = Math.max(wt.branch ? matchRank(task, wt.branch) : 0, matchRank(task, wt.folder));
    if (rank > bestRank) {
      best = wt;
      bestRank = rank;
      if (rank === 2) break;
    }
  }
  return best;
}

const samePath = (a: string, b: string) => a.replace(/\/+$/, '') === b.replace(/\/+$/, '');

/** The terminal already working on `task`: the one sitting in its worktree,
 * else one whose current branch names it. By branch, a bare issue number never
 * counts — a terminal can be in any repo, and too many branches carry digits. */
export function terminalForTask(
  task: BoardTask,
  branches: Record<string, string>,
  terminals: { id: string; cwd: string }[] = [],
  worktree: TaskWorktree | null = null,
): string | null {
  if (worktree) {
    const inside = terminals.find((t) => samePath(t.cwd, worktree.path));
    if (inside) return inside.id;
  }
  let fallback: string | null = null;
  for (const [terminalId, branch] of Object.entries(branches)) {
    const rank = matchRank(task, branch, { numbers: false });
    if (rank === 2) return terminalId;
    if (rank === 1 && !fallback) fallback = terminalId;
  }
  return fallback;
}
