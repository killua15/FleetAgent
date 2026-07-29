import { describe, expect, it } from 'vitest';
import { diffLines, diffStats } from './diffLines';

/**
 * The diff every file view in the app renders. It decides what the user
 * believes changed — a wrong line number or a swallowed removal is a lie told
 * confidently, which is worse than a crash.
 */
describe('diffLines', () => {
  it('numbers old and new lines independently', () => {
    // A removal advances the old side only; an addition advances the new side
    // only. Getting this wrong makes every number after the first hunk drift.
    const lines = diffLines('a\nb\nc\n', 'a\nB\nc\n', Infinity);
    expect(lines).toEqual([
      { type: 'context', text: 'a', oldLine: 1, newLine: 1 },
      { type: 'remove', text: 'b', oldLine: 2 },
      { type: 'add', text: 'B', newLine: 2 },
      { type: 'context', text: 'c', oldLine: 3, newLine: 3 },
    ]);
  });

  it('reads a new file as all additions and a deleted one as all removals', () => {
    // Both are represented as an empty side, which is exactly how the backend
    // reports a file that did not exist or no longer does.
    expect(diffLines('', 'one\ntwo\n', Infinity).every((l) => l.type === 'add')).toBe(true);
    expect(diffLines('one\ntwo\n', '', Infinity).every((l) => l.type === 'remove')).toBe(true);
    expect(diffLines(null, undefined, Infinity)).toEqual([]);
  });

  it('collapses long unchanged runs but keeps context around each change', () => {
    const old = Array.from({ length: 20 }, (_, i) => `line ${i}`).join('\n');
    const next = old.replace('line 10', 'CHANGED');

    const lines = diffLines(old, next, 2);
    const collapsed = lines.filter((l) => l.type === 'collapsed');
    expect(collapsed.length).toBeGreaterThan(0);

    // The two lines either side of the change have to survive, or the diff is
    // unreadable: a changed line with no neighbours says nothing about where
    // it lives.
    const texts = lines.filter((l) => l.type !== 'collapsed').map((l) => (l as { text: string }).text);
    expect(texts).toContain('line 8');
    expect(texts).toContain('line 9');
    expect(texts).toContain('line 11');
    expect(texts).toContain('line 12');
  });

  it('hides the filler at the edges and keeps the context next to the change', () => {
    // Same shape as `git diff`: the file's opening lines are not shown, a
    // marker stands for them, and the lines immediately around the change
    // always survive. Asserting "never collapses at the edges" would be
    // asserting a worse diff.
    const old = Array.from({ length: 12 }, (_, i) => `line ${i}`).join('\n');
    const lines = diffLines(old, old.replace('line 6', 'six'), 2);

    expect(lines[0].type).toBe('collapsed');
    expect(lines[lines.length - 1].type).toBe('collapsed');

    const shown = lines.filter((l) => l.type !== 'collapsed').map((l) => (l as { text: string }).text);
    expect(shown).toEqual(['line 4', 'line 5', 'line 6', 'six', 'line 7', 'line 8']);
  });

  it('degrades instead of freezing on a pathologically large change', () => {
    // The LCS table is O(n·m); past the cell cap it falls back to
    // remove-everything-then-add-everything. Slower would be a hung window.
    const old = Array.from({ length: 800 }, (_, i) => `old ${i}`).join('\n');
    const next = Array.from({ length: 800 }, (_, i) => `new ${i}`).join('\n');

    const started = Date.now();
    const lines = diffLines(old, next, Infinity);
    expect(Date.now() - started).toBeLessThan(2000);
    expect(lines.filter((l) => l.type === 'remove')).toHaveLength(800);
    expect(lines.filter((l) => l.type === 'add')).toHaveLength(800);
  });

  it('treats a trailing newline as a line terminator, not an empty line', () => {
    // `a\nb\n` is two lines. Counting a third empty one shows a phantom
    // addition on every file that ends properly — which is most of them.
    expect(diffLines('a\nb\n', 'a\nb\n', Infinity)).toHaveLength(2);
    expect(diffStats('a\nb\n', 'a\nb\n')).toEqual({ additions: 0, removals: 0 });
  });
});

describe('diffStats', () => {
  it('counts every change, including the ones a collapsed view would hide', () => {
    // Counted with infinite context on purpose: the number in the header must
    // describe the file, not the part of it currently on screen.
    const old = Array.from({ length: 30 }, (_, i) => `line ${i}`).join('\n');
    const next = old.replace('line 2', 'two').replace('line 20', 'twenty');
    expect(diffStats(old, next)).toEqual({ additions: 2, removals: 2 });
  });

  it('counts a rewritten file as its full size', () => {
    expect(diffStats('a\n', 'x\ny\nz\n')).toEqual({ additions: 3, removals: 1 });
  });
});
