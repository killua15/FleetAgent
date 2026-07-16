import type { ReactNode } from 'react';
import { useNavigation } from '../nav/NavigationProvider';
import { Button } from './ui/button';
import styles from './AppTopBar.module.css';

export function AppTopBar({ primaryAction }: { primaryAction?: ReactNode }) {
  const { navigate } = useNavigation();

  return (
    <div className={styles.bar}>
      <span className={styles.wordmark}>⌥ fleet</span>
      <span className={styles.version}>v0.1</span>
      <div className={styles.icons}>
        <Button
          variant="outline"
          className={styles.iconBtn}
          style={{ color: 'var(--acc)' }}
          title="Hierarchy trees"
          onClick={() => navigate('tree-canvas')}
        >
          ⌥
        </Button>
        <Button
          variant="outline"
          className={styles.iconBtn}
          style={{ color: 'var(--teal)' }}
          title="Git panel"
          onClick={() => navigate('git-panel')}
        >
          ⎇
        </Button>
        <Button
          variant="outline"
          className={styles.iconBtn}
          style={{ color: 'var(--mut)' }}
          title="Settings"
          onClick={() => navigate('general-settings')}
        >
          ⚙
        </Button>
      </div>
      <span className={styles.spacer} />
      <Button
        variant="outline"
        className={styles.palette}
        title="Command palette"
        onClick={() => navigate('command-palette')}
      >
        search sessions… <span className={styles.kbd}>⌘K</span>
      </Button>
      {primaryAction}
    </div>
  );
}
