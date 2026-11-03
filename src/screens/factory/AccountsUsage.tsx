import { useCallback, useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../../lib/invoke';
import { toastError } from '../../components/ErrorOverlay';
import { hhmm, type AccountUsage } from '../../types/factory';
import styles from '../Factory.module.css';

/** «hace 3 min» from an ISO time. */
function ago(iso: string | null): string {
  if (!iso) return 'sin lectura';
  const mins = Math.max(0, Math.round((Date.now() - new Date(iso).getTime()) / 60000));
  if (mins < 1) return 'ahora';
  if (mins < 60) return `hace ${mins} min`;
  const h = Math.round(mins / 60);
  return h < 48 ? `hace ${h} h` : `hace ${Math.round(h / 24)} días`;
}

/** The team's accounts' usage, read again by the line as turns and orders
 *  end. Reloads on every `factory-updated` of its project. */
export function useTeamUsage(projectId: string) {
  const [usage, setUsage] = useState<AccountUsage[]>([]);
  const load = useCallback(() => {
    invoke<AccountUsage[]>('factory_usage', { projectId })
      .then(setUsage)
      .catch(() => undefined);
  }, [projectId]);
  useEffect(() => {
    load();
    const off = listen<string>('factory-updated', (e) => {
      if (e.payload === projectId) load();
    });
    return () => {
      off.then((f) => f());
    };
  }, [load, projectId]);
  return { usage, setUsage };
}

/** «Cuentas del equipo»: each account's 5-hour session and week, how old the
 *  reading is, and Claude Code's own warning when it gave one. */
export function AccountsUsage({ projectId, usage, onChange }: { projectId: string; usage: AccountUsage[]; onChange: (u: AccountUsage[]) => void }) {
  const [reading, setReading] = useState(false);
  if (!usage.length) return null;
  const refresh = async () => {
    setReading(true);
    try {
      onChange(await invoke<AccountUsage[]>('factory_usage_refresh', { projectId }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setReading(false);
    }
  };
  return (
    <div className={styles.cardFlat}>
      <div className={styles.cardHead}>
        <span className={styles.sectionLabel}>CUENTAS DEL EQUIPO</span>
        <span className={styles.hint}>se actualiza al terminar cada paso (como mucho cada 3 min por cuenta) y al acabar cada orden</span>
        <span className={styles.spacer} />
        <button className={`${styles.btnGhost} ${styles.sm}`} disabled={reading} title="Lee el uso de todas las cuentas ahora (unos segundos por cuenta de Claude)" onClick={refresh}>
          {reading ? 'leyendo…' : '↻ Leer ahora'}
        </button>
      </div>
      <div className={styles.usageStrip} style={{ padding: '0 16px 12px' }}>
        {usage.map((u) => {
          const refused = u.note?.status === 'rejected';
          const near = u.note?.status === 'allowed_warning';
          const pct = u.session ?? 0;
          return (
            <div key={u.accountId} className={`${styles.usageCard} ${refused ? styles.usageBad : near ? styles.usageWarn : ''}`}>
              <div className={styles.row} style={{ gap: 6 }}>
                <span className={styles.lName}>{u.name}</span>
                <span className={styles.dimTxt}>{u.provider}</span>
                <span className={styles.spacer} />
                <span className={styles.lSub}>{ago(u.sampledAt)}</span>
              </div>
              <div className={styles.row} style={{ gap: 6 }}>
                <span className={styles.lSub}>5 h</span>
                <div className={styles.usageBar} style={{ flex: 1 }}>
                  <span className={pct >= 95 ? styles.usageFull : pct >= 75 ? styles.usageHigh : undefined} style={{ width: `${Math.min(100, pct)}%` }} />
                </div>
                <span className={styles.num}>{u.session != null ? `${u.session.toFixed(0)} %` : '—'}</span>
              </div>
              <span className={styles.lSub}>
                semana {u.weekly != null ? `${u.weekly.toFixed(0)} %` : '—'}
                {u.resetsAt && ` · se renueva a las ${hhmm(u.resetsAt)}`}
              </span>
              {(refused || near) && (
                <span className={refused ? styles.bad : styles.warn}>
                  {refused ? '✗ llegó al límite' : '⚠ cerca del límite'}
                  {u.note?.window === 'seven_day' ? ' de la semana' : ' de 5 h'}
                </span>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
