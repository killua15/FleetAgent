import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { TERMINAL_STATE_META, type TerminalActivity, type TerminalInfo } from '../types/terminal';
import styles from './JumpPalette.module.css';

/** ⌘J — go to a terminal by typing.
 *
 * Separate from ⌘K on purpose. The command palette searches *sessions*, which
 * in a terminal-first app is the thing you look for least; this searches what's
 * actually running. Mixing them would mean the fastest path to "the panel I was
 * just in" goes through a list of things that aren't panels.
 *
 * Lives above the screens rather than inside Terminals so it works from the Git
 * panel or the file viewer — which is exactly when you don't want to navigate
 * home first and then hunt.
 */
export function JumpPalette({ open, onClose, onJump }: { open: boolean; onClose: () => void; onJump: (id: string) => void }) {
  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [activity, setActivity] = useState<Record<string, TerminalActivity>>({});
  const [query, setQuery] = useState('');
  const [highlighted, setHighlighted] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    setQuery('');
    setHighlighted(0);
    invoke<TerminalInfo[]>('list_terminals').then(setTerminals).catch(() => setTerminals([]));
    invoke<TerminalActivity[]>('list_terminal_states')
      .then((list) => setActivity(Object.fromEntries(list.map((a) => [a.id, a]))))
      .catch(() => {});
    const unlisten = listen<TerminalActivity>('terminal-state', (event) => {
      setActivity((prev) => ({ ...prev, [event.payload.id]: event.payload }));
    });
    inputRef.current?.focus();
    return () => {
      unlisten.then((f) => f());
    };
  }, [open]);

  const q = query.trim().toLowerCase();
  const results = useMemo(() => {
    const label = (t: TerminalInfo) => t.displayName ?? t.title;
    const matched = terminals.filter(
      (t) =>
        !q ||
        label(t).toLowerCase().includes(q) ||
        t.cwd.toLowerCase().includes(q) ||
        (t.accountName ?? '').toLowerCase().includes(q) ||
        (activity[t.id]?.title ?? '').toLowerCase().includes(q),
    );
    // Same order as the sidebar: whoever is waiting comes first, because the
    // point of a jump list is to get you to the thing that needs you.
    const rank = (t: TerminalInfo) => (activity[t.id]?.state === 'waiting' ? 0 : activity[t.id]?.state === 'working' ? 1 : 2);
    return matched.sort((a, b) => rank(a) - rank(b));
  }, [terminals, activity, q]);

  if (!open) return null;

  const choose = (index: number) => {
    const target = results[index];
    if (!target) return;
    onJump(target.id);
    onClose();
  };

  const onKeyDown = (e: ReactKeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      setHighlighted((h) => Math.min(h + 1, results.length - 1));
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      setHighlighted((h) => Math.max(h - 1, 0));
    } else if (e.key === 'Enter') {
      e.preventDefault();
      choose(highlighted);
    } else if (e.key === 'Escape') {
      e.preventDefault();
      onClose();
    }
  };

  return (
    <div className={styles.backdrop} onClick={onClose}>
      <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className={styles.input}
          placeholder="jump to a terminal…"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setHighlighted(0);
          }}
          onKeyDown={onKeyDown}
        />
        <div className={styles.results}>
          {results.map((t, i) => {
            const state = activity[t.id]?.state ?? 'idle';
            return (
              <button
                key={t.id}
                className={i === highlighted ? styles.rowActive : styles.row}
                onMouseEnter={() => setHighlighted(i)}
                onClick={() => choose(i)}
              >
                <span className={styles.dot} style={{ background: TERMINAL_STATE_META[state].color }} />
                <span className={state === 'waiting' ? styles.nameAttention : styles.name}>{t.displayName ?? t.title}</span>
                {activity[t.id]?.title && <span className={styles.agentTitle}>{activity[t.id].title}</span>}
                <span className={styles.spacer} />
                {t.accountName && <span className={styles.account}>{t.accountName}</span>}
              </button>
            );
          })}
          {results.length === 0 && <span className={styles.empty}>{terminals.length === 0 ? 'no terminals open' : `nothing matches “${query}”`}</span>}
        </div>
      </div>
    </div>
  );
}
