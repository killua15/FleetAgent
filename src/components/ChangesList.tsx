import type { ChangedFile, WorktreeChanges } from '../types/gitPanel';
import { lineTotals, sameItem, type ReviewItem } from '../lib/review';
import styles from './ChangesList.module.css';

const STATUS_LETTER: Record<ChangedFile['status'], string> = {
  added: 'A',
  modified: 'M',
  deleted: 'D',
  untracked: '?',
  touched: '·',
};

/** A worktree's changes as GitHub Desktop lists them: what the agent is doing
 * right now, then what this branch already has on top of its base. The same
 * file can appear in both, so the group is part of what's selected. */
export function ChangesList({
  changes,
  selected,
  onSelect,
}: {
  changes: WorktreeChanges;
  selected: ReviewItem | null;
  onSelect: (item: ReviewItem) => void;
}) {
  const group = (title: string, hint: string, files: ChangedFile[], committed: boolean) => {
    const sum = lineTotals(files);
    return (
      <div className={styles.section}>
        <div className={styles.sectionHead}>
          <span className={styles.sectionTitle}>{title}</span>
          <span className={styles.sectionHint}>{hint}</span>
          <span className={styles.spacer} />
          {files.length > 0 && (
            <span className={styles.counts}>
              <span className={styles.added}>+{sum.added}</span> <span className={styles.removed}>−{sum.removed}</span>
            </span>
          )}
        </div>
        {files.length === 0 && <span className={styles.sectionEmpty}>nothing</span>}
        {files.map((file) => {
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
              <span className={styles.fileCounts}>
                <span className={styles.added}>+{file.added}</span> <span className={styles.removed}>−{file.removed}</span>
              </span>
            </button>
          );
        })}
      </div>
    );
  };

  return (
    <div className={styles.groups}>
      {group('uncommitted', 'what the agent is doing right now', changes.uncommitted, false)}
      {group('in this branch', `on top of ${changes.base}`, changes.committed, true)}
    </div>
  );
}
