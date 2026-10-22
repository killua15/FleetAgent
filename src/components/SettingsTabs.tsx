import { useNavigation, type ScreenName } from '../nav/NavigationProvider';
import styles from './SettingsTabs.module.css';

// The `shared-memory` tab is deliberately absent: the screen is a complete
// CRUD editor over `memory_files`, but nothing ever feeds that content to an
// agent — no CLAUDE.md is written, no system prompt is appended — so every
// file written there silently has no effect on any session. Hidden rather
// than deleted: the screen, its commands, and its tables all still work, and
// re-adding this entry is the only step needed to bring it back once the
// content actually reaches a turn.
const TABS: Array<{ screen: ScreenName; label: string }> = [
  { screen: 'general-settings', label: 'general' },
  { screen: 'project-settings', label: 'projects' },
  { screen: 'forge-settings', label: 'control de versiones' },
  { screen: 'skill-settings', label: 'skills' },
  { screen: 'graph-groups', label: 'graphs' },
  { screen: 'rotation-settings', label: 'rotation' },
  { screen: 'task-settings', label: 'tasks' },
  { screen: 'docs-settings', label: 'docs' },
];

interface SettingsTabsProps {
  active: ScreenName;
  title: string;
  subtitle?: string;
}

/** Shared header for the three Settings screens — mirrors the mockup's
 * `settings / <Title>` breadcrumb, the general|rotation|memory tab strip,
 * and the "← workspace" link back to MainView. */
export function SettingsTabs({ active, title, subtitle }: SettingsTabsProps) {
  const { navigate } = useNavigation();

  return (
    <div className={styles.header}>
      <span className={styles.crumb}>settings /</span>
      <span className={styles.title}>{title}</span>
      {subtitle && <span className={styles.subtitle}>— {subtitle}</span>}
      <span className={styles.spacer} />
      <div className={styles.tabs}>
        {TABS.map((t) => (
          <button
            key={t.screen}
            className={t.screen === active ? styles.tabActive : styles.tab}
            onClick={() => navigate(t.screen)}
          >
            {t.label}
          </button>
        ))}
      </div>
      {/* Always jumps straight to the terminals home — `back()` would just
       * undo one tab switch (general→rotation all push their own history
       * entry), landing on another settings tab instead of leaving Settings. */}
      <button className={styles.backLink} onClick={() => navigate('terminals')}>
        ← terminals
      </button>
    </div>
  );
}
