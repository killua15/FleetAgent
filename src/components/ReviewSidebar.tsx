import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { reviewItems, type ReviewItem } from '../lib/review';
import type { WorktreeChanges } from '../types/gitPanel';
import type { BranchInfo } from '../types/gitHistory';
import type { TerminalActivity, TerminalInfo } from '../types/terminal';
import { ChangesList } from './ChangesList';
import { DiffOverlay } from './DiffOverlay';
import { Button } from './ui/button';
import styles from './ReviewSidebar.module.css';

/**
 * The review of one terminal's work, next to the terminals: what the agent is
 * changing now and what its branch already has on top of its base. Clicking a
 * file opens the diff over everything, GitHub Desktop style.
 *
 * One shared panel that follows the focused terminal, on purpose — a file list
 * inside every card was tried and pulled back out: too cramped to read, and
 * noise on the cards with nothing to show.
 */
export function ReviewSidebar({ terminal }: { terminal: TerminalInfo | null }) {
  const path = terminal?.cwd ?? null;
  const [changes, setChanges] = useState<WorktreeChanges | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  // `null` = whatever the backend decides (the base the worktree was cut
  // from, when Hangar cut it). A pick here wins, until the terminal changes.
  const [base, setBase] = useState<string | null>(null);
  const [branches, setBranches] = useState<string[]>([]);
  const [selected, setSelected] = useState<ReviewItem | null>(null);
  const [rider, setRider] = useState(false);

  useEffect(() => {
    invoke<boolean>('is_rider_installed').then(setRider).catch(() => setRider(false));
  }, []);

  const load = async () => {
    if (!path) return;
    setLoading(true);
    try {
      setChanges(await invoke<WorktreeChanges>('get_worktree_changes', { path, base }));
      setError(null);
    } catch (e) {
      setChanges(null);
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  // A different terminal is a different review: start from its own base.
  useEffect(() => {
    setBase(null);
    setSelected(null);
    setChanges(null);
    if (!path) return;
    invoke<BranchInfo[]>('list_branches', { path })
      .then((list) => setBranches(list.map((b) => b.name)))
      .catch(() => setBranches([]));
  }, [path]);

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [path, base]);

  // When *this* terminal's agent stops, its files stop moving — the one moment
  // worth re-reading git, instead of polling every worktree on a timer.
  const reload = useRef(load);
  reload.current = load;
  useEffect(() => {
    const unlisten = listen<TerminalActivity>('terminal-state', (event) => {
      if (event.payload.id !== terminal?.id) return;
      if (event.payload.state === 'idle' || event.payload.state === 'waiting') reload.current();
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [terminal?.id]);

  if (!terminal) {
    return (
      <aside className={styles.sidebar}>
        <span className={styles.empty}>Click a terminal to review its changes.</span>
      </aside>
    );
  }

  const shownBase = base ?? changes?.base ?? '';
  const count = changes ? changes.uncommitted.length + changes.committed.length : 0;

  return (
    <aside className={styles.sidebar}>
      <div className={styles.head}>
        <span className={styles.title} title={terminal.cwd}>
          {terminal.displayName ?? terminal.title}
        </span>
        <span className={styles.spacer} />
        <Button size="sm" variant="ghost" disabled={loading} onClick={load} title="Read the changes again">
          {loading ? '…' : '↻'}
        </Button>
      </div>
      {changes && (
        <div className={styles.baseRow}>
          <span className={styles.dim}>vs</span>
          <select
            className={styles.baseSelect}
            value={shownBase}
            onChange={(e) => setBase(e.target.value)}
            title="What this branch's changes are measured against"
          >
            {!branches.includes(shownBase) && <option value={shownBase}>{shownBase}</option>}
            {branches.map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </select>
          {rider && (
            <Button size="sm" variant="outline" onClick={() => invoke('open_in_rider', { path: terminal.cwd }).catch(() => {})}>
              Rider
            </Button>
          )}
        </div>
      )}

      <div className={styles.body}>
        {error && <span className={styles.empty}>This terminal isn't in a git repository.</span>}
        {changes && count === 0 && <span className={styles.empty}>No changes against {changes.base}.</span>}
        {changes && count > 0 && <ChangesList changes={changes} selected={selected} onSelect={setSelected} />}
      </div>

      {changes && selected && path && (
        <DiffOverlay
          path={path}
          base={base}
          item={selected}
          items={reviewItems(changes)}
          rider={rider}
          onSelect={setSelected}
          onClose={() => setSelected(null)}
        />
      )}
    </aside>
  );
}
