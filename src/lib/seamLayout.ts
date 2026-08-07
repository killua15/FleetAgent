import type { Bridge } from '../types/graph';

/** Paths are long and the interesting part is the tail. The full path stays on
 * the node's `<title>`, so nothing is actually lost. */
export function shortenPath(path: string): string {
  return path.replace(/^(src-tauri\/src|lib\/domain\/entities|lib\/presentation\/widgets|lib|src)\//, '');
}

/** Mono at 9.5px: a character is about this wide. Used to size the gutters
 * from the labels that will actually be drawn. */
const CHAR_WIDTH = 5.75;
/** Breathing room between a label and the rail's dot. */
const LABEL_GAP = 22;
/** Horizontal room the curves need to still read as curves. */
const RAIL_SPAN = 360;
const ROW_GAP = 26;
const TOP = 34;
const BOTTOM = 18;

export interface SeamLayout {
  /** Repo on the left rail, and on the right. */
  left: string;
  right: string;
  leftFiles: string[];
  rightFiles: string[];
  /** x of each rail. */
  leftX: number;
  rightX: number;
  width: number;
  height: number;
  rowGap: number;
  top: number;
}

/**
 * Places the seam map's two rails from the labels that will be drawn.
 *
 * The measurement is the whole point. The first version used fixed margins and
 * a fixed viewBox, and **a viewBox clips permanently**: a path longer than the
 * margin was cut off, and no amount of scrolling on the container brought it
 * back, because the container was scrolling an already-cropped image. Sizing
 * the gutters from the longest label is what guarantees every label fits
 * inside the box that gets drawn.
 *
 * Returns `null` when there is no seam to draw — fewer than two repos.
 */
export function seamLayout(bridges: Bridge[]): SeamLayout | null {
  const repos = [...new Set(bridges.flatMap((b) => b.ends.map((e) => e.repo)))].sort();
  const [left, right] = repos;
  if (!left || !right) return null;

  const filesOf = (repo: string) => [
    ...new Set(bridges.flatMap((b) => b.ends.filter((e) => e.repo === repo).map((e) => e.sourceFile))),
  ];
  const leftFiles = filesOf(left);
  const rightFiles = filesOf(right);

  const widest = (files: string[]) => files.reduce((max, f) => Math.max(max, shortenPath(f).length), 0) * CHAR_WIDTH;
  const leftX = Math.ceil(widest(leftFiles)) + LABEL_GAP;
  const rightX = leftX + RAIL_SPAN;

  return {
    left,
    right,
    leftFiles,
    rightFiles,
    leftX,
    rightX,
    width: rightX + Math.ceil(widest(rightFiles)) + LABEL_GAP,
    height: TOP + Math.max(leftFiles.length, rightFiles.length) * ROW_GAP + BOTTOM,
    rowGap: ROW_GAP,
    top: TOP,
  };
}

/** y of a file on its rail. */
export function rowY(layout: SeamLayout, files: string[], file: string): number {
  return layout.top + files.indexOf(file) * layout.rowGap;
}
