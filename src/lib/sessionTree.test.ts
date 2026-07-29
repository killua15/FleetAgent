import { describe, expect, it } from 'vitest';
import type { SessionView } from '../types/session';
import { buildSessionTree, countSessionSubtree } from './sessionTree';

const session = (id: string, parentSessionId: string | null = null): SessionView => ({
  id,
  name: id,
  projectId: 'p1',
  projectName: 'Project',
  parentSessionId,
  worktreeBranch: null,
  worktreeKind: null,
  accountId: 'a1',
  accountName: 'Account',
  provider: 'claude',
  scope: 'personal',
  model: 'opus',
  effort: 'high',
  mode: 'ask_edits',
  status: 'waiting',
  startedAt: '2026-07-29T00:00:00Z',
});

describe('buildSessionTree', () => {
  it('puts each child straight after its parent, indented', () => {
    // The list arrives in arbitrary order from SQLite; the tree is what makes
    // it readable.
    const rows = buildSessionTree([session('child', 'root'), session('root'), session('grandchild', 'child')]);
    expect(rows.map((r) => [r.session.id, r.depth])).toEqual([
      ['root', 0],
      ['child', 1],
      ['grandchild', 2],
    ]);
  });

  it('shows an orphan as its own root rather than hiding it', () => {
    // A subsession whose parent moved to another account, or was deleted, is
    // still real work. Dropping it would make sessions disappear from the UI
    // while still existing in the database.
    const rows = buildSessionTree([session('orphan', 'a-parent-not-in-this-list')]);
    expect(rows).toEqual([{ session: expect.objectContaining({ id: 'orphan' }), depth: 0 }]);
  });

  it('keeps every session exactly once', () => {
    const flat = [session('r1'), session('a', 'r1'), session('b', 'r1'), session('r2'), session('c', 'b')];
    const rows = buildSessionTree(flat);
    expect(rows).toHaveLength(flat.length);
    expect(new Set(rows.map((r) => r.session.id)).size).toBe(flat.length);
  });

  it('handles an empty list', () => {
    expect(buildSessionTree([])).toEqual([]);
  });
});

describe('countSessionSubtree', () => {
  it('counts descendants, not the session itself', () => {
    // The number goes into "this also removes N subsessions", so counting the
    // root would overstate the damage by one on every delete.
    const flat = [session('root'), session('a', 'root'), session('b', 'root'), session('c', 'a')];
    expect(countSessionSubtree(flat, 'root')).toBe(3);
    expect(countSessionSubtree(flat, 'a')).toBe(1);
    expect(countSessionSubtree(flat, 'c')).toBe(0);
  });

  it("is zero for a session that is not in the list", () => {
    expect(countSessionSubtree([session('root')], 'gone')).toBe(0);
  });
});
