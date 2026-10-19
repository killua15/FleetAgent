import { describe, expect, it } from 'vitest';
import type { BoardTask } from '../types/task';
import type { TaskWorktree } from '../types/gitPanel';
import { matchRank, orderedStatuses, terminalForTask, worktreeForTask } from './tasksPanel';

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

  it('never matches a bare issue number by a terminal branch', () => {
    const t = task({ key: '12', suggestedBranch: 'issue-12-x' });
    expect(terminalForTask(t, { a: 'release-12', b: '12-hotfix' })).toBeNull();
  });

  it('finds the terminal sitting in the task worktree', () => {
    const t = task({ key: '128' });
    const wt = worktree({ path: '/r/.hangar/worktrees/issue-128' });
    expect(terminalForTask(t, {}, [{ id: 'x', cwd: '/r' }, { id: 'y', cwd: '/r/.hangar/worktrees/issue-128/' }], wt)).toBe('y');
  });
});

const worktree = (over: Partial<TaskWorktree>): TaskWorktree => ({
  projectId: 'p',
  projectName: 'simulation-server',
  repo: 'o/r',
  path: '/w',
  branch: null,
  folder: 'w',
  sessionId: null,
  ...over,
});

describe('matchRank', () => {
  it('reads an issue number in the shapes people give it, and nowhere else', () => {
    const t = task({ key: '128', suggestedBranch: 'issue-128-fix-login' });
    expect(matchRank(t, 'issue-128-fix-login')).toBe(2);
    for (const name of ['issue-128-other', '128-fix', 'feat/#128', 'gh-128', 'feature/128']) expect(matchRank(t, name)).toBe(1);
    for (const name of ['release-128', 'v1.128', '1280-x', 'fix-128']) expect(matchRank(t, name)).toBe(0);
  });

  it('reads a distinctive key in a branch or a folder', () => {
    const t = task({ key: 'CU-86akmv' });
    expect(matchRank(t, 'cu-86akmv')).toBe(1);
    expect(matchRank(t, 'CU-86akmv_signal-catalog')).toBe(1);
  });
});

describe('worktreeForTask', () => {
  it('only looks in the GitHub issue own repo', () => {
    const t = task({ key: '12', sourceKind: 'github', container: 'o/r' });
    const other = worktree({ repo: 'o/other', branch: 'issue-12-x', path: '/a' });
    const mine = worktree({ repo: 'o/r', branch: 'issue-12-y', path: '/b' });
    expect(worktreeForTask(t, [other, mine])?.path).toBe('/b');
    expect(worktreeForTask(t, [other])).toBeNull();
  });

  it('prefers the exact branch over a loose mention', () => {
    const t = task({ key: 'DEV-42', suggestedBranch: 'DEV-42-thing' });
    const loose = worktree({ branch: 'feature/dev-42-old', path: '/loose' });
    const exact = worktree({ branch: 'DEV-42-thing', path: '/exact' });
    expect(worktreeForTask(t, [loose, exact])?.path).toBe('/exact');
  });

  it('matches by folder name when the branch was renamed', () => {
    const t = task({ key: 'CU-86akmv' });
    expect(worktreeForTask(t, [worktree({ branch: 'renamed', folder: 'cu-86akmv', path: '/f' })])?.path).toBe('/f');
  });
});
