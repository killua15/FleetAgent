import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ChangedFile, WorktreeChanges } from '../types/gitPanel';
import {
  filterChanges,
  lineTotals,
  readDiffLayout,
  readDiffScope,
  readReviewFilter,
  reviewItems,
  statusCounts,
  stepItem,
  writeDiffLayout,
  writeReviewFilter,
} from './review';

const file = (path: string, added = 1, removed = 0): ChangedFile => ({ path, status: 'modified', added, removed, staged: false });
const changes: WorktreeChanges = {
  base: 'dev',
  uncommitted: [file('a.ts'), file('both.ts')],
  committed: [file('both.ts'), file('c.ts')],
};

/** A review walks file by file without closing the diff — the order and the
 * ends decide whether that feels like reading or like hunting. */
describe('review navigation', () => {
  it('reads uncommitted first, then the branch, keeping a file that is in both twice', () => {
    expect(reviewItems(changes).map((i) => `${i.committed ? 'C' : 'U'}:${i.file.path}`)).toEqual(['U:a.ts', 'U:both.ts', 'C:both.ts', 'C:c.ts']);
  });

  it('steps across the two groups and stops at the ends', () => {
    const items = reviewItems(changes);
    expect(stepItem(items, items[1], 1)).toBe(items[2]);
    expect(stepItem(items, items[3], 1)).toBe(items[3]);
    expect(stepItem(items, items[0], -1)).toBe(items[0]);
  });

  it('starts at the first file when nothing is open, and has nothing to step in an empty review', () => {
    expect(stepItem(reviewItems(changes), null, 1)).toEqual(reviewItems(changes)[0]);
    expect(stepItem([], null, 1)).toBeNull();
  });

  it('adds up the lines of a group', () => {
    expect(lineTotals([file('x', 3, 1), file('y', 2, 5)])).toEqual({ added: 5, removed: 6 });
  });
});

describe('diff layout preference', () => {
  afterEach(() => vi.unstubAllGlobals());

  it('remembers split and defaults to unified', () => {
    const store = new Map<string, string>();
    vi.stubGlobal('localStorage', { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) });
    expect(readDiffLayout()).toBe('unified');
    writeDiffLayout('split');
    expect(readDiffLayout()).toBe('split');
  });

  it('falls back to unified when storage is unavailable', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    });
    expect(() => writeDiffLayout('split')).not.toThrow();
    expect(readDiffLayout()).toBe('unified');
  });
});

import { changesTree, folderPaths, orderedReviewItems, orderLikeTree } from './review';

/** The tree is how a review answers "which part of the project did this
 * touch" — nesting, compaction and the per-folder sums are the whole point. */
describe('changesTree', () => {
  const files = [
    file('src/Bridge/Application/Maneuvers/Start.cs', 10, 2),
    file('src/Bridge/Application/Maneuvers/Stop.cs', 4, 1),
    file('src/Bridge/Domain/Maneuver.cs', 20, 5),
    file('README.md', 1, 0),
  ];

  it('nests by folder, folders before files, and compacts single-folder chains', () => {
    const tree = changesTree(files);
    expect(tree.map((n) => n.name)).toEqual(['src/Bridge', 'README.md']);
    const bridge = tree[0];
    expect(bridge.children.map((n) => n.name)).toEqual(['Application/Maneuvers', 'Domain', ]);
    expect(bridge.children[0].children.map((n) => n.name)).toEqual(['Start.cs', 'Stop.cs']);
  });

  it('sums the lines and counts the files under each folder', () => {
    const bridge = changesTree(files)[0];
    expect([bridge.added, bridge.removed, bridge.files]).toEqual([34, 8, 3]);
    expect(bridge.children[0].added).toBe(14);
  });

  it('keeps two groups apart through the prefix', () => {
    const u = changesTree([file('a/x.ts')], 'u:');
    const c = changesTree([file('a/x.ts')], 'c:');
    expect(u[0].path).not.toBe(c[0].path);
    expect(folderPaths(u)).toEqual(['u:a/']);
  });

  it('walks files in the order the tree draws them', () => {
    expect(orderLikeTree(files).map((f) => f.path)).toEqual([
      'src/Bridge/Application/Maneuvers/Start.cs',
      'src/Bridge/Application/Maneuvers/Stop.cs',
      'src/Bridge/Domain/Maneuver.cs',
      'README.md',
    ]);
  });

  it('orders the review like the tree in tree mode and like git in list mode', () => {
    const c: WorktreeChanges = { base: 'dev', uncommitted: [file('z.ts'), file('a/b.ts')], committed: [] };
    expect(orderedReviewItems(c, 'tree').map((i) => i.file.path)).toEqual(['a/b.ts', 'z.ts']);
    expect(orderedReviewItems(c, 'list').map((i) => i.file.path)).toEqual(['z.ts', 'a/b.ts']);
  });
});

/** The list as `git status` and `git diff` would show it: only some kinds of
 * change, or only what isn't committed yet. */
describe('change filters', () => {
  const as = (path: string, status: ChangedFile['status']): ChangedFile => ({ path, status, added: 1, removed: 0, staged: false });
  const mixed: WorktreeChanges = {
    base: 'dev',
    uncommitted: [as('m.ts', 'modified'), as('new.ts', 'untracked'), as('gone.ts', 'deleted')],
    committed: [as('m2.ts', 'modified'), as('added.ts', 'added')],
  };
  const paths = (c: WorktreeChanges) => [...c.uncommitted.map((f) => `U:${f.path}`), ...c.committed.map((f) => `C:${f.path}`)];

  it('keeps everything by default', () => {
    expect(paths(filterChanges(mixed, { scope: 'all', status: 'all' }))).toHaveLength(5);
  });

  it('drops the branch list when only uncommitted work is wanted', () => {
    expect(paths(filterChanges(mixed, { scope: 'uncommitted', status: 'all' }))).toEqual(['U:m.ts', 'U:new.ts', 'U:gone.ts']);
  });

  it('keeps one kind of change in both lists, counting untracked as new', () => {
    expect(paths(filterChanges(mixed, { scope: 'all', status: 'modified' }))).toEqual(['U:m.ts', 'C:m2.ts']);
    expect(paths(filterChanges(mixed, { scope: 'all', status: 'added' }))).toEqual(['U:new.ts', 'C:added.ts']);
    expect(paths(filterChanges(mixed, { scope: 'uncommitted', status: 'deleted' }))).toEqual(['U:gone.ts']);
  });

  it('counts each chip within the chosen scope', () => {
    expect(statusCounts(mixed, 'all')).toEqual({ all: 5, modified: 2, added: 2, deleted: 1 });
    expect(statusCounts(mixed, 'uncommitted')).toEqual({ all: 3, modified: 1, added: 1, deleted: 1 });
  });
});

describe('remembered review preferences', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('defaults to only the changes, and everything in the list, with no storage at all', () => {
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('blocked');
      },
      setItem: () => {
        throw new Error('blocked');
      },
    });
    expect(readDiffScope()).toBe('changes');
    expect(readReviewFilter()).toEqual({ scope: 'all', status: 'all' });
    expect(() => writeReviewFilter({ scope: 'uncommitted', status: 'modified' })).not.toThrow();
  });

  it('remembers a filter, and ignores one it does not recognise', () => {
    const store = new Map<string, string>();
    vi.stubGlobal('localStorage', { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) });
    writeReviewFilter({ scope: 'uncommitted', status: 'modified' });
    expect(readReviewFilter()).toEqual({ scope: 'uncommitted', status: 'modified' });
    localStorage.setItem('hangar.review.filter', JSON.stringify({ scope: 'weird', status: 'renamed' }));
    expect(readReviewFilter()).toEqual({ scope: 'all', status: 'all' });
  });
});
