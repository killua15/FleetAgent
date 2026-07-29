import type { BoardTask } from '../types/task';

/** What the task board is filtered down to. `all` is a real value rather than
 * an absent one: an unset filter and "every value" are the same thing here,
 * and modelling them differently is how a screen ends up with two ways to mean
 * nothing. */
export interface TaskFilters {
  sourceId: string;
  /** The board's own project — a ClickUp list, a GitHub repo. */
  container: string;
  status: string;
}

export const NO_FILTER = 'all';

export const EMPTY_FILTERS: TaskFilters = { sourceId: NO_FILTER, container: NO_FILTER, status: NO_FILTER };

/**
 * The board's filtering, out of the component so it can be tested.
 *
 * The order matters and isn't arbitrary: the source is applied first, and the
 * container and status options are derived from what that source returned.
 * ClickUp lists mean nothing in a GitHub repo, so offering them together would
 * present choices that can only ever match zero tasks.
 */
export function filterTasks(tasks: BoardTask[], filters: TaskFilters): BoardTask[] {
  return bySource(tasks, filters.sourceId).filter(
    (task) =>
      (filters.container === NO_FILTER || task.container === filters.container) &&
      (filters.status === NO_FILTER || task.status === filters.status),
  );
}

export function bySource(tasks: BoardTask[], sourceId: string): BoardTask[] {
  return sourceId === NO_FILTER ? tasks : tasks.filter((task) => task.sourceId === sourceId);
}

/** The container values actually present, sorted. Derived from the data rather
 * than from a fixed list because these are the user's own ClickUp lists and
 * repos — nothing can be hardcoded that would be right for two people. */
export function containerOptions(tasks: BoardTask[], sourceId: string): string[] {
  return distinct(bySource(tasks, sourceId).map((task) => task.container));
}

/** Likewise for status: ClickUp statuses are invented per space
 * ("verificar", "pronto para teste"), so any fixed list would be right for
 * exactly one board. */
export function statusOptions(tasks: BoardTask[], sourceId: string): string[] {
  return distinct(bySource(tasks, sourceId).map((task) => task.status));
}

function distinct(values: (string | null | undefined)[]): string[] {
  return [...new Set(values.filter((value): value is string => Boolean(value)))].sort();
}

/** How many of each option would survive, for the counts beside them. */
export function countBy(tasks: BoardTask[], pick: (task: BoardTask) => string | null): Record<string, number> {
  const counts: Record<string, number> = {};
  for (const task of tasks) {
    const key = pick(task);
    if (key) counts[key] = (counts[key] ?? 0) + 1;
  }
  return counts;
}
