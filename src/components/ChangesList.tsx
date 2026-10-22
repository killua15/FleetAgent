import { useMemo, useState } from 'react';
import type { ChangedFile, WorktreeChanges } from '../types/gitPanel';
import { changesTree, folderPaths, lineTotals, sameItem, type ChangeNode, type ListLayout, type ReviewItem } from '../lib/review';
import { FileTreeRows } from './FileTree';
import styles from './ChangesList.module.css';

const STATUS_LETTER: Record<ChangedFile['status'], string> = {
  added: 'A',
  modified: 'M',
  deleted: 'D',
  untracked: '?',
  touched: '·',
};

/** The tree paths of the two groups, so one file in both stays two rows. */
const GROUP_PREFIX = { uncommitted: 'u:', committed: 'c:' } as const;

/** A worktree's changes as GitHub Desktop lists them: what the agent is doing
 * right now, then what this branch already has on top of its base — as a
 * folder tree (which part of the project this touched) or a flat list. The
 * same file can appear in both groups, so the group is part of what's
 * selected. */
export function ChangesList({
  changes,
  selected,
  onSelect,
  layout = 'tree',
  showCommitted = true,
}: {
  changes: WorktreeChanges;
  selected: ReviewItem | null;
  onSelect: (item: ReviewItem) => void;
  layout?: ListLayout;
  /** False when only uncommitted work is wanted — the branch's group isn't
   * drawn at all, rather than drawn empty. */
  showCommitted?: boolean;
}) {
  // Collapsed folders survive a refresh (this component stays mounted); a new
  // terminal remounts it and starts expanded.
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set());

  const trees = useMemo(
    () => ({
      uncommitted: changesTree(changes.uncommitted, GROUP_PREFIX.uncommitted),
      committed: changesTree(changes.committed, GROUP_PREFIX.committed),
    }),
    [changes],
  );

  const toggle = (path: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const setAll = (nodes: ChangeNode[], collapse: boolean) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      for (const path of folderPaths(nodes)) {
        if (collapse) next.add(path);
        else next.delete(path);
      }
      return next;
    });

  const counts = (added: number, removed: number) => (
    <span className={styles.fileCounts}>
      <span className={styles.added}>+{added}</span> <span className={styles.removed}>−{removed}</span>
    </span>
  );

  const flat = (files: ChangedFile[], committed: boolean) =>
    files.map((file) => {
      const item = { file, committed };
      return (
        <button
          key={`${committed}-${file.path}`}
          className={sameItem(item, selected) ? styles.fileActive : styles.file}
          onClick={() => onSelect(item)}
          title={file.path}
        >
          <span className={`${styles.status} ${styles[file.status]}`}>{STATUS_LETTER[file.status]}</span>
          {/* The name is what you scan for; the directory is context and
              gets truncated from the left, where the useless part is. */}
          <span className={styles.fileName}>{file.path.split('/').pop()}</span>
          <span className={styles.fileDir}>{file.path.split('/').slice(0, -1).join('/')}</span>
          {counts(file.added, file.removed)}
        </button>
      );
    });

  const tree = (nodes: ChangeNode[], committed: boolean) => {
    const prefix = committed ? GROUP_PREFIX.committed : GROUP_PREFIX.uncommitted;
    const selectedPath = selected && selected.committed === committed ? `${prefix}${selected.file.path}` : null;
    return (
      <FileTreeRows
        nodes={nodes}
        selectedPath={selectedPath}
        collapsedPaths={collapsed}
        onToggleCollapsed={toggle}
        onSelectFile={(path) => {
          const file = (committed ? changes.committed : changes.uncommitted).find((f) => `${prefix}${f.path}` === path);
          if (file) onSelect({ file, committed });
        }}
        renderMeta={(node) => (
          <span className={styles.treeMeta}>
            {node.isDir && <span className={styles.treeCount}>{node.files}</span>}
            {counts(node.added, node.removed)}
          </span>
        )}
      />
    );
  };

  const group = (title: string, hint: string, files: ChangedFile[], committed: boolean) => {
    const sum = lineTotals(files);
    const nodes = committed ? trees.committed : trees.uncommitted;
    const hasFolders = layout === 'tree' && nodes.some((n) => n.isDir);
    return (
      <div className={styles.section}>
        <div className={styles.sectionHead}>
          <span className={styles.sectionTitle}>{title}</span>
          <span className={styles.sectionHint}>{hint}</span>
          <span className={styles.spacer} />
          {hasFolders && (
            <>
              <button className={styles.headAction} onClick={() => setAll(nodes, true)} title="Collapse every folder">
                ▸
              </button>
              <button className={styles.headAction} onClick={() => setAll(nodes, false)} title="Expand every folder">
                ▾
              </button>
            </>
          )}
          {files.length > 0 && (
            <span className={styles.counts}>
              <span className={styles.added}>+{sum.added}</span> <span className={styles.removed}>−{sum.removed}</span>
            </span>
          )}
        </div>
        {files.length === 0 && <span className={styles.sectionEmpty}>nothing</span>}
        {files.length > 0 && (layout === 'tree' ? tree(nodes, committed) : flat(files, committed))}
      </div>
    );
  };

  return (
    <div className={styles.groups}>
      {group('uncommitted', 'what the agent is doing right now', changes.uncommitted, false)}
      {showCommitted && group('in this branch', `on top of ${changes.base}`, changes.committed, true)}
    </div>
  );
}
