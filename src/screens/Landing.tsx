import { useState } from 'react';
import { useNavigation } from '../nav/NavigationProvider';
import { Button } from '../components/ui/button';
import styles from './Landing.module.css';

const PROVIDERS = [
  { letter: 'C', label: 'claude code', color: 'var(--acc)' },
  { letter: 'X', label: 'codex cli', color: 'var(--teal)' },
  { letter: 'G', label: 'gemini cli', color: 'var(--gemini)' },
  { letter: '𝕏', label: 'grok cli', color: 'var(--grok)' },
];

export function Landing() {
  const [entering, setEntering] = useState(false);
  const { navigate } = useNavigation();

  const handleEnter = () => {
    if (entering) return;
    setEntering(true);
    window.setTimeout(() => navigate('terminals'), 450);
  };

  return (
    <div className={styles.landing}>
      <div className={styles.trafficLights}>
        <span style={{ background: '#f45c57' }} />
        <span style={{ background: '#f0b62f' }} />
        <span style={{ background: '#39c451' }} />
      </div>

      <div className={styles.content}>
        <div className={styles.mark}>⌥</div>
        <div className={styles.heading}>
          <span className={styles.wordmark}>hangar</span>
          <span className={styles.tagline}>
            Mission control for your coding agents. Open a real Claude Code terminal in any project,
            under any account — one at a time or a whole grid at once, without ever mixing personal
            and work.
          </span>
        </div>

        <div className={styles.providers}>
          {PROVIDERS.map((p) => (
            <span key={p.label} className={styles.providerPill}>
              <span style={{ color: p.color }}>{p.letter}</span> {p.label}
            </span>
          ))}
        </div>

        <Button
          type="button"
          className={`${styles.enterBtn} ${entering ? styles.entering : ''}`}
          onClick={handleEnter}
          title="Open Hangar"
        >
          {entering ? 'Opening terminals…' : 'Enter Hangar →'}
        </Button>
        <span className={styles.footNote}>local-first · your keys stay in the keychain</span>
      </div>

      <div className={styles.version}>hangar v0.1 — {navigator.platform.includes('Mac') ? 'macOS' : 'desktop'} · Tauri</div>
    </div>
  );
}
