import { useState } from 'react';
import { openUrl } from '@tauri-apps/plugin-opener';
import { orderChip, type FactoryOrder } from '../../types/factory';
import styles from '../Factory.module.css';

const CHIP: Record<string, string> = { acc: styles.chipAcc, red: styles.chipRed, amber: styles.chipAmber, teal: styles.chipTeal, grn: styles.chipGrn, '': styles.chip };

/**
 * The project's queue in the order ▶ takes it. Drag (or ↑ ↓) to reorder the
 * orders still to do; an order waits for the ones it depends on.
 */
export function QueueList({
  orders,
  providerShort,
  onReorder,
  onRetry,
}: {
  orders: FactoryOrder[];
  providerShort: string;
  onReorder?: (ids: string[]) => void;
  onRetry?: (orderId: string) => void;
}) {
  const [dragId, setDragId] = useState<string | null>(null);
  const [overId, setOverId] = useState<string | null>(null);
  const done = new Set(orders.filter((o) => o.status === 'merged').map((o) => o.key));
  const nextId = orders.find((o) => o.status === 'todo' && o.blockedBy.length === 0)?.id;

  const move = (id: string, to: number) => {
    const ids = orders.map((o) => o.id).filter((x) => x !== id);
    ids.splice(Math.max(0, Math.min(to, ids.length)), 0, id);
    onReorder?.(ids);
  };

  if (orders.length === 0) return <span className={styles.hint}>La cola está vacía.</span>;

  return (
    <div className={styles.col} style={{ gap: 6 }}>
      {orders.map((o, i) => {
        const [tone, label] = orderChip(o, o.id === nextId);
        const movable = !!onReorder && o.status === 'todo';
        return (
          <div
            key={o.id}
            className={`${styles.qItem} ${overId === o.id ? styles.qOver : ''} ${dragId === o.id ? styles.qDragging : ''}`}
            draggable={movable}
            onDragStart={() => setDragId(o.id)}
            onDragEnd={() => {
              setDragId(null);
              setOverId(null);
            }}
            onDragOver={(e) => {
              if (!dragId) return;
              e.preventDefault();
              setOverId(o.id);
            }}
            onDragLeave={() => setOverId(null)}
            onDrop={(e) => {
              e.preventDefault();
              if (dragId && dragId !== o.id) move(dragId, i);
              setDragId(null);
              setOverId(null);
            }}
          >
            <span className={styles.grip} title={movable ? 'Arrastra para ordenar' : undefined}>
              {movable ? '⠿' : ''}
            </span>
            <span className={styles.qKey}>{o.key}</span>
            <span className={styles.qTitle} title={o.description || undefined}>
              {o.extUrl ? (
                <button className={styles.qLink} onClick={() => o.extUrl && openUrl(o.extUrl)}>
                  {o.title}
                </button>
              ) : (
                o.title
              )}
            </span>
            <span className={styles.qDeps}>{o.deps.length ? `⤷ ${o.deps.map((d) => (done.has(d) ? `${d} ✓` : d)).join(' · ')}` : ''}</span>
            <span className={styles.chip}>{o.extId ? providerShort : 'local'}</span>
            <span className={CHIP[tone]}>{label}</span>
            {onRetry && (o.status === 'failed' || o.status === 'stopped') && (
              <button className={`${styles.btn} ${styles.sm}`} onClick={() => onRetry(o.id)}>
                Reintentar
              </button>
            )}
            {movable && (
              <span className={styles.qMove}>
                <button className={styles.btnGhost} disabled={i === 0} title="subir" onClick={() => move(o.id, i - 1)}>
                  ↑
                </button>
                <button className={styles.btnGhost} disabled={i === orders.length - 1} title="bajar" onClick={() => move(o.id, i + 1)}>
                  ↓
                </button>
              </span>
            )}
          </div>
        );
      })}
    </div>
  );
}
