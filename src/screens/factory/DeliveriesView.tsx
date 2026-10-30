import { openUrl } from '@tauri-apps/plugin-opener';
import type { Delivery } from '../../types/factory';
import styles from '../Factory.module.css';

const STATUS: Record<Delivery['status'], [string, string]> = {
  merged: [styles.chipGrn, 'fusionada'],
  delivered: [styles.chipTeal, 'MR abierta'],
  running: [styles.chipAcc, 'en curso'],
  waiting_limit: [styles.chipAmber, 'en espera'],
  waiting_human: [styles.chipAmber, '✋ te espera'],
  stopped: [styles.chip, 'parada'],
  failed: [styles.chipRed, 'fallida'],
};

/** Entregas: every order the line took, how it was delivered, the rounds it
 *  needed (and for which rules), the rules it cited and what it cost. */
export function DeliveriesView({ deliveries }: { deliveries: Delivery[] }) {
  if (!deliveries.length) return <span className={styles.hint}>Todavía no hay entregas.</span>;
  return (
    <div className={styles.cardFlat}>
      <div className={styles.dHead}>
        <span>Orden</span>
        <span>Entrega</span>
        <span>Estado</span>
        <span>Rondas</span>
        <span>Reglas citadas</span>
        <span>Coste</span>
      </div>
      {deliveries.map((d) => {
        const [cls, label] = STATUS[d.status];
        return (
          <div key={d.runId} className={styles.dRow}>
            <span className={styles.qKey}>{d.orderKey}</span>
            <span>
              <b>{d.title}</b>{' '}
              <span className={styles.dimTxt}>
                ·{' '}
                {d.mrUrl ? (
                  <button className={styles.qLink} onClick={() => openUrl(d.mrUrl!)}>
                    MR → {d.mrUrl.split('/').slice(-2).join('/')}
                  </button>
                ) : d.status === 'merged' ? (
                  'merge local'
                ) : (
                  d.error ?? `en ${d.stage}`
                )}
              </span>
            </span>
            <span>
              <span className={cls}>{label}</span>
            </span>
            <span className={styles.num}>{d.rounds ? `${d.rounds}${d.roundRules.length ? ` (${d.roundRules.join(', ')})` : ''}` : '—'}</span>
            <span className={styles.num}>{d.rulesCited}</span>
            <span className={styles.num}>${d.costUsd.toFixed(2)}</span>
          </div>
        );
      })}
    </div>
  );
}
