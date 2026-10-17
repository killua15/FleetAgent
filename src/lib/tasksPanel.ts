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

/** The terminal already working on `task`, judged by the branch its folder is
 * on: the task's own branch, the one its board suggests, or — for a key that
 * can't be mistaken for anything else, like `DEV-42` — a branch that names it.
 * A bare issue number (`128`) is never matched loosely: too many branches
 * contain digits. */
export function terminalForTask(task: BoardTask, branches: Record<string, string>): string | null {
  const exact = [task.suggestedBranch, task.branchHint].filter(Boolean).map((b) => b!.toLowerCase());
  const loose = /^\d+$/.test(task.key) ? null : task.key.toLowerCase();
  let fallback: string | null = null;
  for (const [terminalId, branch] of Object.entries(branches)) {
    const name = branch.toLowerCase();
    if (exact.includes(name)) return terminalId;
    if (loose && !fallback && name.split(/[/_\s]/).some((part) => part === loose || part.startsWith(`${loose}-`))) fallback = terminalId;
  }
  return fallback;
}
