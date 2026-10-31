import { useState } from 'react';
import type { FactoryRun, GateResult } from '../../types/factory';
import styles from '../Factory.module.css';

export type CheckAction = 'fix' | 'broken' | 'waive';

/** The test gate's last verdict, one row per command, and what to do with a
 *  red one: back to the builder, to the brain's inbox, or ignored here. */
export function GatePanel({ run, onCheck }: { run: FactoryRun; onCheck: (action: CheckAction, check: string | null, note?: string) => void }) {
  const gate = run.state.gate;
  const red = gate.filter((g) => !g.ok);
  const [open, setOpen] = useState<string | null>(red[0]?.command ?? null);
  const [note, setNote] = useState('');
  // While the line works on the order, its state is the worker's.
  const busy = run.status === 'running';

  if (!gate.length) return null;
  return (
    <div id="factory-gate" className={styles.cardFlat}>
      <div className={styles.cardHead}>
        <span className={styles.sectionLabel}>
          PUERTA DE TESTS · {gate.length - red.length} DE {gate.length} EN VERDE
        </span>
        <span className={styles.spacer} />
        {red.length > 1 && !busy && (
          <button className={`${styles.btn} ${styles.sm}`} title="El builder recibe la salida de todos los rojos" onClick={() => onCheck('fix', null, note)}>
            Mandar los {red.length} rojos a Fabricación
          </button>
        )}
      </div>
      {red.length > 0 &&
        (busy ? (
          <div className={`${styles.hint} ${styles.gateNote}`}>La orden está en marcha: el builder ya recibe estos rojos. Para actuar sobre un check, páusala o espera a que se pare.</div>
        ) : (
          <div className={styles.gateNote}>
            <input className={styles.input} placeholder="nota para el builder (opcional)" value={note} onChange={(e) => setNote(e.target.value)} />
          </div>
        ))}
      {(run.state.waived.length > 0 || run.state.broken.length > 0) && (
        <div className={`${styles.hint} ${styles.gateNote}`}>
          No corren en esta orden:
          {run.state.waived.length > 0 && <> ignorados {run.state.waived.join(', ')}</>}
          {run.state.waived.length > 0 && run.state.broken.length > 0 && ' ·'}
          {run.state.broken.length > 0 && <> en la bandeja del brain {run.state.broken.join(', ')} (vuelven al sellar y reintentar)</>}
        </div>
      )}
      <div className={styles.gateList}>
        {[...red, ...gate.filter((g) => g.ok)].map((g) => (
          <GateRow key={g.command} g={g} busy={busy} open={open === g.command} onToggle={() => setOpen(open === g.command ? null : g.command)} onCheck={(a) => onCheck(a, g.command, note)} />
        ))}
      </div>
    </div>
  );
}

function GateRow({ g, busy, open, onToggle, onCheck }: { g: GateResult; busy: boolean; open: boolean; onToggle: () => void; onCheck: (action: CheckAction) => void }) {
  // A tester's command is the suite: only the builder can fix it. Brain
  // checks are named by their rule ids.
  const brainOnly = g.name.split(', ').every((n) => /^[A-Z]+-\d+$/.test(n));
  return (
    <div className={styles.gateRow}>
      <button className={styles.gateHead} onClick={onToggle} disabled={g.ok}>
        <span className={g.ok ? styles.ok : styles.bad}>{g.ok ? '✓' : '✗'}</span>
        <span className={styles.lName}>{g.name}</span>
        <span className={`${styles.lSub} ${styles.gateCmd}`}>{g.command}</span>
        {!g.ok && g.reds > 1 && <span className={styles.chipAmber}>rojo {g.reds} rondas seguidas</span>}
      </button>
      {open && !g.ok && (
        <div className={styles.gateBody}>
          {g.rule && <div className={styles.hint}>{g.rule}</div>}
          <pre className={styles.gateOut}>{g.tail || '(sin salida)'}</pre>
          {!busy && (
            <div className={styles.gateActions}>
              <button
                className={`${styles.btnPrimary} ${styles.sm}`}
                title="El builder recibe esta salida y tu nota; una orden fallida empieza sus rondas de nuevo"
                onClick={() => onCheck('fix')}
              >
                Arreglar en Fabricación
              </button>
              {brainOnly ? (
                <>
                  <button
                    className={`${styles.btn} ${styles.sm}`}
                    title="A la bandeja del Brain; no corre en esta orden hasta que lo arregles, selles y reintentes"
                    onClick={() => onCheck('broken')}
                  >
                    El check está mal
                  </button>
                  <button className={`${styles.btnGhost} ${styles.sm}`} title="No corre en esta orden; queda en los avisos y en el mensaje de la entrega" onClick={() => onCheck('waive')}>
                    Ignorar en esta orden
                  </button>
                </>
              ) : (
                <span className={styles.hint}>Es el comando de un tester: la suite no se ignora. Si el comando está mal, cámbialo en Equipo.</span>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
