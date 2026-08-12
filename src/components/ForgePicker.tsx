import { ALL_HOSTS, forgeHostLabel, type ForgeHostOption } from '../lib/forgeHosts';
import styles from './ForgePicker.module.css';

/**
 * Which forge instance the list below is scoped to — GitHub, GitLab, or a
 * self-hosted instance by name.
 *
 * Renders nothing when there's one host or fewer, and that's the point: the
 * overwhelming majority of setups are a single GitHub, and a control that can
 * only ever select what's already selected is noise on their screen. It
 * appears the moment a second instance shows up on its own.
 */
export function ForgePicker({
  options,
  selected,
  onSelect,
  total,
}: {
  options: ForgeHostOption[];
  selected: string;
  onSelect: (host: string) => void;
  /** How many items "All" would show — including any the forges couldn't be
   * worked out for, which is why this isn't the sum of the options' counts. */
  total: number;
}) {
  if (options.length < 2) return null;

  return (
    <div className={styles.picker}>
      <button
        className={selected === ALL_HOSTS ? styles.optionActive : styles.option}
        onClick={() => onSelect(ALL_HOSTS)}
        title="Every provider"
      >
        All
        <span className={styles.count}>{total}</span>
      </button>
      {options.map((option) => (
        <button
          key={option.host}
          className={selected === option.host ? styles.optionActive : styles.option}
          onClick={() => onSelect(option.host)}
          title={option.host}
        >
          {forgeHostLabel(option)}
          <span className={styles.count}>{option.count}</span>
        </button>
      ))}
    </div>
  );
}
