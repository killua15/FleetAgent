import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { invoke } from '../lib/invoke';
import { useNavigation } from '../nav/NavigationProvider';
import { useTheme } from '../theme/ThemeProvider';
import { PROVIDER_META } from '../types/account';
import type { SessionView } from '../types/session';
import styles from './CommandPalette.module.css';

interface ActionItem {
  id: string;
  icon: string;
  name: string;
  kbd: string;
  run: () => void;
}

type FlatItem = { kind: 'session'; session: SessionView } | { kind: 'action'; action: ActionItem };

export function CommandPalette() {
  const { navigate, back } = useNavigation();
  const { toggleTheme } = useTheme();
  const [sessions, setSessions] = useState<SessionView[]>([]);
  const [query, setQuery] = useState('');
  const [highlighted, setHighlighted] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    invoke<SessionView[]>('list_workspace_sessions').then(setSessions).catch(() => {});
    inputRef.current?.focus();
  }, []);

  // Escape closes regardless of where focus is (e.g. after hovering a row),
  // not just while the search input itself has focus.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') back();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [back]);

  const q = query.trim().toLowerCase();

  const filteredSessions = useMemo(
    () =>
      sessions.filter(
        (s) => !q || s.name.toLowerCase().includes(q) || s.projectName.toLowerCase().includes(q) || s.accountName.toLowerCase().includes(q),
      ),
    [sessions, q],
  );

  // Opens the terminals home, where creating a terminal (single or multi)
  // actually lives, rather than duplicating that flow here.
  const actions: ActionItem[] = useMemo(() => {
    const list: ActionItem[] = [{ id: 'new-terminal', icon: '⌗', name: 'New terminal…', kbd: '⌘N', run: () => navigate('terminals') }];
    list.push({ id: 'toggle-theme', icon: '◐', name: 'Toggle theme', kbd: '⌘⇧L', run: () => toggleTheme() });
    return list.filter((a) => !q || a.name.toLowerCase().includes(q));
  }, [q, navigate, toggleTheme]);

  const flatItems: FlatItem[] = useMemo(
    () => [...filteredSessions.map((session): FlatItem => ({ kind: 'session', session })), ...actions.map((action): FlatItem => ({ kind: 'action', action }))],
    [filteredSessions, actions],
  );

  useEffect(() => {
    setHighlighted(0);
  }, [query]);

  const openSession = (s: SessionView) => navigate('agent-panel', { sessionId: s.id });

  const activate = (index: number) => {
    const item = flatItems[index];
    if (!item) return;
    if (item.kind === 'session') openSession(item.session);
    else item.action.run();
  };

  const onInputKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHighlighted((i) => Math.min(i + 1, flatItems.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHighlighted((i) => Math.max(i - 1, 0));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      activate(highlighted);
    }
  };

  return (
    <div className={styles.screen} onClick={back}>
      <div className={styles.card} onClick={(e) => e.stopPropagation()}>
        <div className={styles.searchRow}>
          <span className={styles.searchIcon}>⌕</span>
          <input
            ref={inputRef}
            className={styles.searchInput}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onInputKeyDown}
            placeholder="Jump to session, project, or action…"
            autoFocus
          />
          <span className={styles.escBadge}>esc</span>
        </div>

        <div className={styles.results}>
          {filteredSessions.length > 0 && <div className={styles.sectionLabel}>SESSIONS</div>}
          {filteredSessions.map((s, i) => {
            const isChild = s.parentSessionId !== null;
            return (
              <button
                key={s.id}
                className={i === highlighted ? styles.rowActive : styles.row}
                onMouseEnter={() => setHighlighted(i)}
                onClick={() => openSession(s)}
              >
                <span className={`${styles.dot} ${s.status === 'running' ? styles.dotLive : ''}`} />
                <span className={styles.rowName}>
                  {isChild ? '↳ ' : ''}
                  {s.name || '(unnamed)'}
                </span>
                <span className={styles.rowProject}>{s.projectName}</span>
                <span className={styles.spacer} />
                <span className={styles.rowAccount} style={{ color: PROVIDER_META[s.provider].colorVar }}>
                  {s.accountName}
                </span>
                <span className={styles.rowStatus}>{s.status}</span>
              </button>
            );
          })}

          {actions.length > 0 && <div className={styles.sectionLabel}>ACTIONS</div>}
          {actions.map((a, i) => {
            const index = filteredSessions.length + i;
            return (
              <button
                key={a.id}
                className={index === highlighted ? styles.rowActive : styles.row}
                onMouseEnter={() => setHighlighted(index)}
                onClick={() => activate(index)}
              >
                <span className={styles.actionIcon}>{a.icon}</span>
                <span className={styles.actionName}>{a.name}</span>
                <span className={styles.spacer} />
                <span className={styles.rowStatus}>{a.kbd}</span>
              </button>
            );
          })}

          {flatItems.length === 0 && <span className={styles.emptyText}>No matches.</span>}
        </div>

        <div className={styles.footer}>
          <span>↑↓ navigate</span>
          <span>↵ open</span>
          <span className={styles.spacer} />
          <span>
            {filteredSessions.length} of {sessions.length} sessions
          </span>
        </div>
      </div>
    </div>
  );
}
