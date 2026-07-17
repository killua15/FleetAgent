import { useNavigation, type ScreenName } from '../nav/NavigationProvider';
import styles from './SettingsTabs.module.css';

const TABS: Array<{ screen: ScreenName; label: string }> = [
  { screen: 'general-settings', label: 'general' },
  { screen: 'rotation-settings', label: 'rotation' },
  { screen: 'shared-memory', label: 'memory' },
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
  const { navigate, back, canGoBack } = useNavigation();

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
      <button className={styles.backLink} onClick={() => (canGoBack ? back() : navigate('main-view'))}>
        ← workspace
      </button>
    </div>
  );
}
