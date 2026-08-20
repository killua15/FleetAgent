import { useEffect, useState } from 'react';
import { useNavigation, type ScreenName } from '../nav/NavigationProvider';
import { invoke } from '../lib/invoke';
import { Button } from '../components/ui/button';
import styles from './Landing.module.css';

const PROVIDERS = [
  { letter: 'C', label: 'claude code', color: 'var(--acc)' },
  { letter: 'X', label: 'codex cli', color: 'var(--teal)' },
  { letter: 'G', label: 'gemini cli', color: 'var(--gemini)' },
  { letter: '𝕏', label: 'grok cli', color: 'var(--grok)' },
];

// `navigator.platform` is deprecated but still the only synchronous answer, and
// this is a footer label — not worth an async Tauri call. Anything that isn't a
// Mac or a Windows box stays 'desktop' rather than guessing.
function platformName() {
  const platform = navigator.platform;
  if (platform.includes('Mac')) return 'macOS';
  if (platform.startsWith('Win')) return 'Windows';
  return 'desktop';
}

export function Landing() {
  const [entering, setEntering] = useState(false);
  const { navigate } = useNavigation();
  // Where the door leads. Someone with no accounts yet cannot do anything on
  // the terminal grid — it has nothing to run under — so the first launch opens
  // the checklist instead. Only "no accounts at all" is a safe signal for
  // "new here"; anything cleverer would keep greeting a regular user.
  const [destination, setDestination] = useState<ScreenName>('terminals');

  useEffect(() => {
    invoke<unknown[]>('list_accounts')
      .then((accounts) => setDestination(accounts.length === 0 ? 'start-here' : 'terminals'))
      .catch(() => {});
  }, []);

  const handleEnter = () => {
    if (entering) return;
    setEntering(true);
    window.setTimeout(() => navigate(destination), 450);
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
          {entering ? 'Opening…' : 'Enter Hangar →'}
        </Button>
        <span className={styles.footNote}>local-first · your keys stay in the keychain</span>
      </div>

      <div className={styles.version}>hangar v0.1 — {platformName()} · Tauri</div>
    </div>
  );
}
