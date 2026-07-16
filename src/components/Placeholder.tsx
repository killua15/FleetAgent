import { useNavigation, type ScreenName } from '../nav/NavigationProvider';
import { Button } from './ui/button';
import styles from './Placeholder.module.css';

export function Placeholder({ screen }: { screen: ScreenName }) {
  const { back, canGoBack } = useNavigation();
  return (
    <div className={styles.wrap}>
      <span className={styles.tag}>Coming soon</span>
      <span className={styles.title}>{screen}</span>
      {canGoBack && (
        <Button variant="outline" className="mt-3.5" onClick={back}>
          ← back
        </Button>
      )}
    </div>
  );
}
