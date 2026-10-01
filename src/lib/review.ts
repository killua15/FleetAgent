import type { ChangedFile, WorktreeChanges } from '../types/gitPanel';

/** One file in the review, and which list it's from — the same file can be in
 * both (committed, then edited again) with different content. */
export interface ReviewItem {
  file: ChangedFile;
  committed: boolean;
}

/** Both lists in reading order: what the agent is doing now, then what the
 * branch already has. The order the keyboard walks. */
export function reviewItems(changes: WorktreeChanges | null): ReviewItem[] {
  if (!changes) return [];
  return [
    ...changes.uncommitted.map((file) => ({ file, committed: false })),
    ...changes.committed.map((file) => ({ file, committed: true })),
  ];
}

export function sameItem(a: ReviewItem | null, b: ReviewItem | null): boolean {
  return !!a && !!b && a.committed === b.committed && a.file.path === b.file.path;
}

/** The previous or next file, stopping at the ends rather than wrapping — a
 * review that loops back to the first file reads as "there's more". */
export function stepItem(items: ReviewItem[], current: ReviewItem | null, dir: 1 | -1): ReviewItem | null {
  if (items.length === 0) return null;
  const at = items.findIndex((item) => sameItem(item, current));
  if (at === -1) return items[0];
  return items[Math.min(Math.max(at + dir, 0), items.length - 1)];
}

export function lineTotals(files: ChangedFile[]): { added: number; removed: number } {
  return files.reduce((sum, f) => ({ added: sum.added + f.added, removed: sum.removed + f.removed }), { added: 0, removed: 0 });
}

export type DiffLayout = 'unified' | 'split';
const LAYOUT_KEY = 'hangar.review.diffLayout';

/** The diff layout the user last picked. Storage can be unavailable (private
 * mode, a preview); the default is what GitHub Desktop shows. */
export function readDiffLayout(): DiffLayout {
  try {
    return localStorage.getItem(LAYOUT_KEY) === 'split' ? 'split' : 'unified';
  } catch {
    return 'unified';
  }
}

export function writeDiffLayout(layout: DiffLayout) {
  try {
    localStorage.setItem(LAYOUT_KEY, layout);
  } catch {
    // Remembering is a convenience; failing to is not worth a toast.
  }
}
