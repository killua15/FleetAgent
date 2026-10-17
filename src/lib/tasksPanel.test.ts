import { describe, expect, it } from 'vitest';
import type { BoardTask } from '../types/task';
import { orderedStatuses, terminalForTask } from './tasksPanel';

const task = (over: Partial<BoardTask>): BoardTask => ({
  externalId: '1',
  key: '1',
  title: 't',
  description: '',
  url: '',
  status: null,
  statusKind: null,
  branchHint: null,
  sourceId: 's',
  sourceKind: 'clickup',
  container: null,
  suggestedBranch: '',
  ...over,
});

describe('orderedStatuses', () => {
  it('puts open work first and finished work last, with counts', () => {
    const tasks = [
      task({ status: 'verificar', statusKind: 'custom' }),
      task({ status: 'done', statusKind: 'done' }),
      task({ status: 'to do', statusKind: 'open' }),
      task({ status: 'verificar', statusKind: 'custom' }),
      task({ status: null }),
    ];
    expect(orderedStatuses(tasks)).toEqual([
      { status: 'to do', count: 1 },
      { status: 'verificar', count: 2 },
      { status: 'done', count: 1 },
    ]);
  });
});

describe('terminalForTask', () => {
  it('matches the branch the task would be cut on', () => {
    const t = task({ key: '128', suggestedBranch: 'issue-128-fix-login' });
    expect(terminalForTask(t, { a: 'main', b: 'issue-128-fix-login' })).toBe('b');
  });

  it('finds a hand-made branch that names a distinctive key', () => {
    const t = task({ key: 'DEV-42', suggestedBranch: 'DEV-42-thing' });
    expect(terminalForTask(t, { a: 'feature/dev-42-other-name' })).toBe('a');
  });

  it('never matches a bare issue number loosely', () => {
    const t = task({ key: '12', suggestedBranch: 'issue-12-x' });
    expect(terminalForTask(t, { a: 'release-12', b: '12-hotfix' })).toBeNull();
  });
});
