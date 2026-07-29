import { describe, expect, it } from 'vitest';
import type { BoardTask } from '../types/task';
import { containerOptions, countBy, EMPTY_FILTERS, filterTasks, NO_FILTER, statusOptions } from './taskFilters';

const task = (overrides: Partial<BoardTask>): BoardTask => ({
  externalId: 'x',
  key: 'X-1',
  title: 'a task',
  description: '',
  url: '',
  status: null,
  branchHint: null,
  sourceId: 'clickup',
  sourceKind: 'clickup',
  container: null,
  suggestedBranch: 'x-1-a-task',
  ...overrides,
});

// Shaped after the real board: two accounts, ClickUp lists and a GitHub repo,
// and statuses invented per space.
const board: BoardTask[] = [
  task({ externalId: '1', sourceId: 'clickup', container: 'Orquestrador', status: 'backlog' }),
  task({ externalId: '2', sourceId: 'clickup', container: 'Orquestrador', status: 'em andamento' }),
  task({ externalId: '3', sourceId: 'clickup', container: 'Viewer', status: 'backlog' }),
  task({ externalId: '4', sourceId: 'github', sourceKind: 'github', container: 'wfo/api', status: 'open' }),
  task({ externalId: '5', sourceId: 'clickup', container: null, status: null }),
];

describe('filterTasks', () => {
  it('shows everything when nothing is chosen', () => {
    expect(filterTasks(board, EMPTY_FILTERS)).toHaveLength(5);
  });

  it('narrows by source, then by list, then by status', () => {
    expect(filterTasks(board, { ...EMPTY_FILTERS, sourceId: 'github' })).toHaveLength(1);
    expect(filterTasks(board, { ...EMPTY_FILTERS, container: 'Orquestrador' })).toHaveLength(2);
    expect(filterTasks(board, { ...EMPTY_FILTERS, container: 'Orquestrador', status: 'backlog' })).toHaveLength(1);
  });

  it('keeps a task with no list or status until one is asked for', () => {
    // A ClickUp task can genuinely have neither. Dropping it from the
    // unfiltered view would hide real work; keeping it under a specific
    // filter would answer a question it doesn't match.
    const unfiltered = filterTasks(board, EMPTY_FILTERS).map((t) => t.externalId);
    expect(unfiltered).toContain('5');
    expect(filterTasks(board, { ...EMPTY_FILTERS, status: 'backlog' }).map((t) => t.externalId)).not.toContain('5');
  });

  it('can filter down to nothing without breaking', () => {
    // The screen has to tell "no tasks" apart from "no matches", so returning
    // an empty array here is the correct answer, not a bug to route around.
    expect(filterTasks(board, { ...EMPTY_FILTERS, sourceId: 'github', container: 'Viewer' })).toEqual([]);
  });
});

describe('option lists', () => {
  it('offers only the values the chosen source actually has', () => {
    // The bug this prevents: showing ClickUp's lists while GitHub is selected,
    // so every option matches zero tasks.
    expect(containerOptions(board, 'github')).toEqual(['wfo/api']);
    expect(containerOptions(board, 'clickup')).toEqual(['Orquestrador', 'Viewer']);
    expect(statusOptions(board, 'github')).toEqual(['open']);
  });

  it('deduplicates and sorts, and drops the empty ones', () => {
    expect(containerOptions(board, NO_FILTER)).toEqual(['Orquestrador', 'Viewer', 'wfo/api']);
    // The task with no container contributes no option — an empty entry in a
    // dropdown is unselectable noise.
    expect(containerOptions(board, NO_FILTER)).not.toContain('');
  });

  it('returns nothing for a source with no tasks rather than the whole list', () => {
    expect(containerOptions(board, 'a-source-with-nothing')).toEqual([]);
    expect(statusOptions([], NO_FILTER)).toEqual([]);
  });
});

describe('countBy', () => {
  it('counts each value and ignores the absent ones', () => {
    expect(countBy(board, (t) => t.container)).toEqual({ Orquestrador: 2, Viewer: 1, 'wfo/api': 1 });
    expect(countBy(board, (t) => t.status)).toEqual({ backlog: 2, 'em andamento': 1, open: 1 });
  });

  it('is empty for an empty board', () => {
    expect(countBy([], (t) => t.status)).toEqual({});
  });
});
