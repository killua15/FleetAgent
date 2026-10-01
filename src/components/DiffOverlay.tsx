import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { readDiffLayout, stepItem, writeDiffLayout, type DiffLayout, type ReviewItem } from '../lib/review';
import { CodeView } from './CodeView';
import { toastError } from './ErrorOverlay';
import { Button } from './ui/button';
import styles from './DiffOverlay.module.css';

/**
 * One file's diff over the whole screen, the terminals left running underneath.
 *
 * A review reads file after file, so the keyboard walks the list without
 * closing this (↑/↓ or k/j), and Esc gives the terminals back.
 */
export function DiffOverlay({
  path,
  base,
  item,
  items,
  rider,
  onSelect,
  onClose,
}: {
  path: string;
  base: string | null;
  item: ReviewItem;
  items: ReviewItem[];
  rider: boolean;
  onSelect: (item: ReviewItem) => void;
  onClose: () => void;
}) {
  const [diff, setDiff] = useState<{ oldText: string; newText: string } | null>(null);
  const [layout, setLayout] = useState<DiffLayout>(readDiffLayout);

  useEffect(() => {
    let cancelled = false;
    setDiff(null);
    invoke<{ oldText: string; newText: string }>('get_worktree_file_diff', {
      path,
      filePath: item.file.path,
      base,
      committed: item.committed,
    })
      .then((found) => {
        if (!cancelled) setDiff(found);
      })
      .catch((e) => toastError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [path, base, item.file.path, item.committed]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      if (target && (target.tagName === 'INPUT' || target.tagName === 'SELECT')) return;
      let handled = true;
      if (e.key === 'Escape') onClose();
      else if (e.key === 'ArrowDown' || e.key === 'j') {
        const next = stepItem(items, item, 1);
        if (next) onSelect(next);
      } else if (e.key === 'ArrowUp' || e.key === 'k') {
        const prev = stepItem(items, item, -1);
        if (prev) onSelect(prev);
      } else handled = false;
      // Kept from reaching xterm: these keys mean "read the next file" here,
      // not "send an arrow to the shell".
      if (handled) {
        e.preventDefault();
        e.stopPropagation();
      }
    };
    window.addEventListener('keydown', onKey, true);
    return () => window.removeEventListener('keydown', onKey, true);
  }, [items, item, onSelect, onClose]);

  const pick = (next: DiffLayout) => {
    setLayout(next);
    writeDiffLayout(next);
  };
  const position = items.findIndex((i) => i.file.path === item.file.path && i.committed === item.committed) + 1;

  return (
    <div className={styles.backdrop} onClick={onClose}>
      <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
        <div className={styles.head}>
          <span className={styles.path} title={item.file.path}>
            {item.file.path}
          </span>
          <span className={styles.group}>{item.committed ? 'in this branch' : 'uncommitted'}</span>
          <span className={styles.counts}>
            <span className={styles.added}>+{item.file.added}</span> <span className={styles.removed}>−{item.file.removed}</span>
          </span>
          <span className={styles.spacer} />
          <span className={styles.hint}>
            {position}/{items.length} · ↑↓ files · Esc closes
          </span>
          <div className={styles.toggle}>
            <button className={layout === 'unified' ? styles.toggleOn : styles.toggleOff} onClick={() => pick('unified')}>
              Unified
            </button>
            <button className={layout === 'split' ? styles.toggleOn : styles.toggleOff} onClick={() => pick('split')}>
              Side by side
            </button>
          </div>
          {rider && (
            <Button size="sm" variant="outline" onClick={() => invoke('open_in_rider', { path: `${path}/${item.file.path}` }).catch(() => {})}>
              Rider
            </Button>
          )}
          <Button size="sm" variant="ghost" onClick={onClose} title="Close (Esc)">
            ✕
          </Button>
        </div>
        <div className={styles.body}>
          {!diff && <span className={styles.loading}>Reading…</span>}
          {diff && item.file.status === 'deleted' && !diff.newText && !diff.oldText && (
            <span className={styles.loading}>This file was deleted, and git has no earlier copy to show.</span>
          )}
          {diff && (diff.newText || diff.oldText) && (
            <CodeView path={item.file.path} text={diff.newText} original={diff.oldText} split={layout === 'split'} />
          )}
        </div>
      </div>
    </div>
  );
}
