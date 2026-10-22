import { statusCounts, type ReviewFilter } from '../lib/review';
import type { WorktreeChanges } from '../types/gitPanel';
import styles from './ChangesFilterBar.module.css';

const STATUS_CHIPS: { id: ReviewFilter['status']; letter: string; label: string }[] = [
  { id: 'modified', letter: 'M', label: 'modified' },
  { id: 'added', letter: 'A', label: 'new' },
  { id: 'deleted', letter: 'D', label: 'deleted' },
];

/**
 * The change list's filters, the way git splits the question: everything this
 * branch did or only what isn't committed yet (`git status`), and every kind of
 * change or one. Shared by the ⇄ Changes panel and the Changes screen so the
 * two read the same.
 */
export function ChangesFilterBar({
  changes,
  filter,
  onChange,
}: {
  changes: WorktreeChanges;
  filter: ReviewFilter;
  onChange: (next: ReviewFilter) => void;
}) {
  const counts = statusCounts(changes, filter.scope);
  return (
    <div className={styles.bar}>
      <div className={styles.toggle}>
        <button
          className={filter.scope === 'all' ? styles.toggleOn : styles.toggleOff}
          onClick={() => onChange({ ...filter, scope: 'all' })}
          title="What the agent is doing now, and what this branch already has"
        >
          Everything
        </button>
        <button
          className={filter.scope === 'uncommitted' ? styles.toggleOn : styles.toggleOff}
          onClick={() => onChange({ ...filter, scope: 'uncommitted' })}
          title="Only what isn't committed yet — what git status lists"
        >
          Uncommitted only
        </button>
      </div>
      <div className={styles.chips}>
        <button className={filter.status === 'all' ? styles.chipOn : styles.chip} onClick={() => onChange({ ...filter, status: 'all' })}>
          All <span className={styles.count}>{counts.all}</span>
        </button>
        {STATUS_CHIPS.filter((c) => counts[c.id] > 0 || filter.status === c.id).map((c) => (
          <button
            key={c.id}
            className={filter.status === c.id ? styles.chipOn : styles.chip}
            onClick={() => onChange({ ...filter, status: c.id })}
            title={`Only ${c.label} files`}
          >
            <span className={`${styles.letter} ${styles[c.id]}`}>{c.letter}</span> {c.label} <span className={styles.count}>{counts[c.id]}</span>
          </button>
        ))}
      </div>
    </div>
  );
}
