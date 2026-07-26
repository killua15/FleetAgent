/** Lane-assignment layout for a commit DAG — the same idea `git log --graph`
 * uses internally to decide which column each commit's dot sits in and where
 * the connecting lines run, so the git panel can draw real branch/merge
 * paths instead of a flat list. Pure and synchronous: feed it a page of
 * commits (newest first, as `list_commits` already returns them) and render
 * the result — no state, no git calls. */

export interface GraphCommit {
  hash: string;
  parents: string[];
}

export interface GraphRow {
  hash: string;
  /** Column this commit's dot sits in. */
  lane: number;
  /** Columns that just draw a plain vertical line through this row,
   * unrelated to this commit. */
  passthroughLanes: number[];
  /** Columns this commit's line continues down into, below this row — one
   * entry per parent still being tracked (empty for a root commit). */
  parentLanes: number[];
  /** Extra columns (beyond `lane`) that converge into this commit from
   * above — a fork point, where more than one earlier row pointed at this
   * same commit as their next parent. */
  mergeFromLanes: number[];
  /** Total lanes in play once this row is processed — the max across all
   * rows is what the renderer needs for the graph column's width. */
  laneCount: number;
}

export function layoutCommitGraph(commits: GraphCommit[]): GraphRow[] {
  const lanes: Array<string | null> = [];
  const rows: GraphRow[] = [];

  for (const commit of commits) {
    const waitingLanes: number[] = [];
    for (let i = 0; i < lanes.length; i++) {
      if (lanes[i] === commit.hash) waitingLanes.push(i);
    }

    let lane: number;
    if (waitingLanes.length > 0) {
      lane = waitingLanes[0];
    } else {
      const free = lanes.indexOf(null);
      lane = free === -1 ? lanes.length : free;
    }
    if (lane === lanes.length) lanes.push(null);

    const passthroughLanes: number[] = [];
    for (let i = 0; i < lanes.length; i++) {
      if (i !== lane && lanes[i] !== null && !waitingLanes.includes(i)) passthroughLanes.push(i);
    }

    const mergeFromLanes = waitingLanes.slice(1);
    for (const extra of mergeFromLanes) lanes[extra] = null;

    const [firstParent, ...restParents] = commit.parents;
    lanes[lane] = firstParent ?? null;

    const parentLanes: number[] = firstParent ? [lane] : [];
    for (const parent of restParents) {
      let parentLane = lanes.indexOf(parent);
      if (parentLane === -1) {
        parentLane = lanes.indexOf(null);
        if (parentLane === -1) {
          parentLane = lanes.length;
          lanes.push(null);
        }
        lanes[parentLane] = parent;
      }
      parentLanes.push(parentLane);
    }

    rows.push({ hash: commit.hash, lane, passthroughLanes, parentLanes, mergeFromLanes, laneCount: lanes.length });
  }

  return rows;
}

/** Cycles through Hangar's existing palette tokens rather than introducing
 * new colors — same set already used for ahead/behind/status badges
 * elsewhere in the git panel. */
export const LANE_COLORS = ['var(--acc)', 'var(--teal)', 'var(--amber)', 'var(--grn)', 'var(--red)', 'var(--mut)'];

export function laneColor(lane: number): string {
  return LANE_COLORS[lane % LANE_COLORS.length];
}
