import { describe, expect, it } from 'vitest';
import { laneColor, layoutCommitGraph, LANE_COLORS, type GraphCommit } from './commitGraph';

/** `git log --graph` in the git panel. It's pure geometry over a DAG, and it's
 * the kind of code that looks right on the two commits you tried it with and
 * draws lines into nowhere on a real repository. */
const commit = (hash: string, ...parents: string[]): GraphCommit => ({ hash, parents });

describe('layoutCommitGraph', () => {
  it('keeps a linear history in one lane', () => {
    const rows = layoutCommitGraph([commit('c', 'b'), commit('b', 'a'), commit('a')]);
    expect(rows.map((r) => r.lane)).toEqual([0, 0, 0]);
    expect(rows.every((r) => r.passthroughLanes.length === 0)).toBe(true);
    // The root has no parent, so nothing continues below it.
    expect(rows[2].parentLanes).toEqual([]);
  });

  it('gives a merge its second parent a lane of its own', () => {
    //   m ── merge of `a` (mainline) and `f` (a branch)
    //   |\
    //   a f
    //   |/
    //   r
    const rows = layoutCommitGraph([commit('m', 'a', 'f'), commit('a', 'r'), commit('f', 'r'), commit('r')]);

    const merge = rows[0];
    expect(merge.lane).toBe(0);
    expect(merge.parentLanes).toHaveLength(2);
    expect(new Set(merge.parentLanes).size).toBe(2);
    // A second column has to exist for the branch to be drawn in at all.
    expect(merge.laneCount).toBeGreaterThanOrEqual(2);
  });

  it('marks the fork point where two lines converge', () => {
    // `r` is the parent of both `a` and `f`, so two lanes arrive at it — that
    // convergence is what the renderer draws as the branch closing.
    const rows = layoutCommitGraph([commit('m', 'a', 'f'), commit('a', 'r'), commit('f', 'r'), commit('r')]);
    const root = rows[3];
    expect(root.hash).toBe('r');
    expect(root.mergeFromLanes.length).toBeGreaterThan(0);
  });

  it('draws a passthrough for a branch that is waiting, not for the commit itself', () => {
    // While `a` is being drawn, `f`'s lane is still open below and must show a
    // plain vertical line — without it the branch visually disappears and
    // reappears two rows later.
    const rows = layoutCommitGraph([commit('m', 'a', 'f'), commit('a', 'r'), commit('f', 'r'), commit('r')]);
    const rowA = rows[1];
    expect(rowA.hash).toBe('a');
    expect(rowA.passthroughLanes).not.toContain(rowA.lane);
    expect(rowA.passthroughLanes.length).toBeGreaterThan(0);
  });

  it('reuses a freed lane instead of drifting right forever', () => {
    // Two branches that open and close in turn must not leave the graph five
    // columns wide — the panel's graph column has a fixed budget.
    const rows = layoutCommitGraph([
      commit('m1', 'a', 'f'),
      commit('a', 'r1'),
      commit('f', 'r1'),
      commit('r1', 'm2'),
      commit('m2', 'b', 'g'),
      commit('b', 'r2'),
      commit('g', 'r2'),
      commit('r2'),
    ]);
    expect(Math.max(...rows.map((r) => r.laneCount))).toBeLessThanOrEqual(3);
  });

  it('survives a page that starts mid-history', () => {
    // `list_commits` paginates, so the first row's parents are routinely not
    // in the page and the last row's parents are always missing. Neither may
    // produce a dangling lane or a crash.
    const rows = layoutCommitGraph([commit('c', 'unknown-parent'), commit('b', 'also-missing')]);
    expect(rows).toHaveLength(2);
    expect(rows.every((r) => Number.isInteger(r.lane) && r.lane >= 0)).toBe(true);
  });

  it('handles an empty page', () => {
    expect(layoutCommitGraph([])).toEqual([]);
  });
});

describe('laneColor', () => {
  it('cycles rather than running off the end of the palette', () => {
    expect(laneColor(0)).toBe(LANE_COLORS[0]);
    expect(laneColor(LANE_COLORS.length)).toBe(LANE_COLORS[0]);
    expect(laneColor(LANE_COLORS.length * 3 + 2)).toBe(LANE_COLORS[2]);
  });
});
