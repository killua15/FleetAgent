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

/** A node of the review tree: a folder (with the sum of what's under it) or a
 * changed file. Shaped like `FileTreeNode` so the shared tree renders it. */
export interface ChangeNode {
  /** Unique within one tree. Prefixed by group (`u:` / `c:`) by the caller
   * when two trees share a selection, since a file can be in both. */
  path: string;
  name: string;
  isDir: boolean;
  status: ChangedFile['status'] | null;
  children: ChangeNode[];
  added: number;
  removed: number;
  /** Files at or under this node. */
  files: number;
  change?: ChangedFile;
}

/**
 * The changed files as a folder tree: folders before files, each alphabetical,
 * and a folder with a single folder inside collapsed into one row
 * (`src/components`) the way VS Code compacts them — a C# change five folders
 * deep shouldn't cost five rows before you see a file.
 */
export function changesTree(files: ChangedFile[], prefix = ''): ChangeNode[] {
  interface Dir {
    dirs: Map<string, Dir>;
    files: ChangedFile[];
  }
  const root: Dir = { dirs: new Map(), files: [] };
  for (const file of files) {
    const parts = file.path.split('/');
    let dir = root;
    for (const part of parts.slice(0, -1)) {
      if (!dir.dirs.has(part)) dir.dirs.set(part, { dirs: new Map(), files: [] });
      dir = dir.dirs.get(part)!;
    }
    dir.files.push(file);
  }

  const build = (dir: Dir, parentPath: string): ChangeNode[] => {
    const folders = [...dir.dirs.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([name, child]) => {
        // Compact a chain of single-folder folders into one row.
        let label = name;
        let path = parentPath ? `${parentPath}/${name}` : name;
        let node = child;
        while (node.files.length === 0 && node.dirs.size === 1) {
          const [nextName, next] = [...node.dirs.entries()][0];
          label = `${label}/${nextName}`;
          path = `${path}/${nextName}`;
          node = next;
        }
        const children = build(node, path);
        return {
          path: `${prefix}${path}/`,
          name: label,
          isDir: true,
          status: null,
          children,
          added: children.reduce((sum, c) => sum + c.added, 0),
          removed: children.reduce((sum, c) => sum + c.removed, 0),
          files: children.reduce((sum, c) => sum + c.files, 0),
        } satisfies ChangeNode;
      });
    const leaves = [...dir.files]
      .sort((a, b) => a.path.localeCompare(b.path))
      .map(
        (file): ChangeNode => ({
          path: `${prefix}${file.path}`,
          name: file.path.split('/').pop() ?? file.path,
          isDir: false,
          status: file.status,
          children: [],
          added: file.added,
          removed: file.removed,
          files: 1,
          change: file,
        }),
      );
    return [...folders, ...leaves];
  };
  return build(root, '');
}

/** Every folder path in a tree — what "collapse all" collapses. */
export function folderPaths(nodes: ChangeNode[]): string[] {
  return nodes.flatMap((n) => (n.isDir ? [n.path, ...folderPaths(n.children)] : []));
}

/** The files in the order the tree shows them, so ↓ in the diff moves to the
 * next file you see rather than the next one git listed. */
export function orderLikeTree(files: ChangedFile[]): ChangedFile[] {
  const walk = (nodes: ChangeNode[]): ChangedFile[] => nodes.flatMap((n) => (n.isDir ? walk(n.children) : n.change ? [n.change] : []));
  return walk(changesTree(files));
}

export type ListLayout = 'tree' | 'list';
const LIST_LAYOUT_KEY = 'hangar.review.listLayout';

export function readListLayout(): ListLayout {
  try {
    return localStorage.getItem(LIST_LAYOUT_KEY) === 'list' ? 'list' : 'tree';
  } catch {
    return 'tree';
  }
}

export function writeListLayout(layout: ListLayout) {
  try {
    localStorage.setItem(LIST_LAYOUT_KEY, layout);
  } catch {
    // A convenience; not remembering it is fine.
  }
}

/** `reviewItems` in the order the list is drawn: the tree's order when it's a
 * tree, git's when it's a flat list. */
export function orderedReviewItems(changes: WorktreeChanges | null, layout: ListLayout): ReviewItem[] {
  if (!changes || layout === 'list') return reviewItems(changes);
  return [
    ...orderLikeTree(changes.uncommitted).map((file) => ({ file, committed: false })),
    ...orderLikeTree(changes.committed).map((file) => ({ file, committed: true })),
  ];
}

/** Whether a diff shows only what changed (folded like `git diff`) or the
 * whole file with the changes marked. */
export type DiffScope = 'changes' | 'file';
const DIFF_SCOPE_KEY = 'hangar.review.diffScope';

/** Only the changes unless the user asked for the whole file — a review is
 * read change by change, and a 900-line file with a 2-line edit buries it. */
export function readDiffScope(): DiffScope {
  try {
    return localStorage.getItem(DIFF_SCOPE_KEY) === 'file' ? 'file' : 'changes';
  } catch {
    return 'changes';
  }
}

export function writeDiffScope(scope: DiffScope) {
  try {
    localStorage.setItem(DIFF_SCOPE_KEY, scope);
  } catch {
    // A convenience; not remembering it is fine.
  }
}

/** What the change list shows: both groups or only what `git status` would
 * list, and every kind of change or one. */
export interface ReviewFilter {
  scope: 'all' | 'uncommitted';
  status: 'all' | 'modified' | 'added' | 'deleted';
}

export const ALL_CHANGES: ReviewFilter = { scope: 'all', status: 'all' };
const FILTER_KEY = 'hangar.review.filter';

/** Which chip a file falls under. An untracked file is "new" to whoever reads
 * the list, the same as a staged addition; a file only Hangar saw an agent
 * touch has no git status and stays under "all". */
function statusGroup(status: ChangedFile['status']): ReviewFilter['status'] | null {
  switch (status) {
    case 'modified':
      return 'modified';
    case 'added':
    case 'untracked':
      return 'added';
    case 'deleted':
      return 'deleted';
    default:
      return null;
  }
}

/** The changes the list should draw. `uncommitted` empties the branch's list;
 * a status keeps only that kind in both. */
export function filterChanges(changes: WorktreeChanges, filter: ReviewFilter): WorktreeChanges {
  const keep = (files: ChangedFile[]) => (filter.status === 'all' ? files : files.filter((f) => statusGroup(f.status) === filter.status));
  return {
    ...changes,
    uncommitted: keep(changes.uncommitted),
    committed: filter.scope === 'uncommitted' ? [] : keep(changes.committed),
  };
}

/** How many files each chip would show, within the chosen scope. */
export function statusCounts(changes: WorktreeChanges, scope: ReviewFilter['scope']): Record<ReviewFilter['status'], number> {
  const files = scope === 'uncommitted' ? changes.uncommitted : [...changes.uncommitted, ...changes.committed];
  const counts = { all: files.length, modified: 0, added: 0, deleted: 0 };
  for (const file of files) {
    const group = statusGroup(file.status);
    if (group && group !== 'all') counts[group] += 1;
  }
  return counts;
}

export function readReviewFilter(): ReviewFilter {
  try {
    const saved = JSON.parse(localStorage.getItem(FILTER_KEY) ?? 'null') as Partial<ReviewFilter> | null;
    return {
      scope: saved?.scope === 'uncommitted' ? 'uncommitted' : 'all',
      status: saved?.status === 'modified' || saved?.status === 'added' || saved?.status === 'deleted' ? saved.status : 'all',
    };
  } catch {
    return ALL_CHANGES;
  }
}

export function writeReviewFilter(filter: ReviewFilter) {
  try {
    localStorage.setItem(FILTER_KEY, JSON.stringify(filter));
  } catch {
    // A convenience; not remembering it is fine.
  }
}
