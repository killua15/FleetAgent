import styles from './Spinner.module.css';

/**
 * Small inline "working on it" indicator — for any async action or fetch
 * whose duration isn't guessable (a `gh` round-trip, a delete that touches
 * disk/worktrees, a provider CLI call). Pair with a label so the wait has
 * words, not just motion: `<Spinner label="loading…" />`.
 */
export function Spinner({ label, size = 13 }: { label?: string; size?: number }) {
  return (
    <span className={styles.wrap}>
      <span className={styles.ring} style={{ width: size, height: size }} />
      {label && <span className={styles.label}>{label}</span>}
    </span>
  );
}
