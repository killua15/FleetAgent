import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ChangedFile, WorktreeChanges } from '../types/gitPanel';
import { lineTotals, readDiffLayout, reviewItems, stepItem, writeDiffLayout } from './review';

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
