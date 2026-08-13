import { ALL_HOSTS, forgeHostLabel, type ForgeHostOption } from '../lib/forgeHosts';
import styles from './ForgePicker.module.css';

/**
 * Which forge instance the list below is scoped to — GitHub, GitLab, or a
 * self-hosted instance by name.
 *
 * With "All" on offer it renders nothing for a single host, and that's the
 * point: the overwhelming majority of setups are a single GitHub, and a control
 * that can only ever select what's already selected is noise on their screen.
 * It appears the moment a second instance shows up on its own.
 *
 * Without "All" (`allowAll={false}`) that rule inverts: there is no selection
 * until someone makes one, so even a lone instance has to be offered.
 */
export function ForgePicker({
  options,
  selected,
  onSelect,
  total,
  allowAll = true,
  countTitle,
}: {
  options: ForgeHostOption[];
  selected: string;
  onSelect: (host: string) => void;
  /** How many items "All" would show — including any the forges couldn't be
   * worked out for, which is why this isn't the sum of the options' counts.
   * Ignored when `allowAll` is false. */
  total?: number;
  /** Whether "every provider at once" is on offer.
   *
   * False on the PR screen, where the list is fetched per instance rather than
   * fetched whole and filtered: an "All" there would ask every forge over the
   * network, which is the thing that screen exists to stop doing. */
  allowAll?: boolean;
  /** What the number on each option counts. The badge is the same either way,
   * but it means loaded items on a screen that has loaded some and projects on
   * one that deliberately hasn't — and an unexplained number that changes
   * meaning between screens is worse than no number. */
  countTitle?: string;
}) {
  // With "All" gone the picker is how the choice gets made in the first place,
  // so it has to appear even for a single instance — the hiding rule above only
  // holds while there is something already selected to fall back on.
  if (options.length < (allowAll ? 2 : 1)) return null;

  return (
    <div className={styles.picker}>
      {allowAll && (
        <button
          className={selected === ALL_HOSTS ? styles.optionActive : styles.option}
          onClick={() => onSelect(ALL_HOSTS)}
          title="Every provider"
        >
          All
          <span className={styles.count}>{total}</span>
        </button>
      )}
      {options.map((option) => (
        <button
          key={option.host}
          className={selected === option.host ? styles.optionActive : styles.option}
          onClick={() => onSelect(option.host)}
          title={option.host}
        >
          {forgeHostLabel(option)}
          <span className={styles.count} title={countTitle}>
            {option.count}
          </span>
        </button>
      ))}
    </div>
  );
}
