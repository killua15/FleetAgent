import { useState } from 'react';
import { invoke } from '../../lib/invoke';
import { toastError } from '../../components/ErrorOverlay';
import { TerminalPane } from '../../components/TerminalPane';
import type { Account } from '../../types/account';
import type { FactoryRun, FailItem } from '../../types/factory';
import type { TerminalInfo } from '../../types/terminal';
import type { CheckAction } from './GatePanel';
import styles from '../Factory.module.css';

/** The fix console per run. The PTY outlives this panel, so leaving «En vivo»
 *  and coming back finds the same conversation. */
const consoles = new Map<string, string>();

/** «Por qué se paró»: an order that ran out of rounds, said plainly — what
 *  stayed red at the end, what each round sent back, and a console in its
 *  worktree to fix it by hand. */
export function FailurePanel({
  run,
  orderKey,
  accounts,
  onResolve,
  onCheck,
}: {
  run: FactoryRun;
  orderKey: string;
  accounts: Account[];
  onResolve: (action: string) => void;
  onCheck: (action: CheckAction, check: string | null, note?: string) => void;
}) {
  const history = run.state.history ?? [];
  const last = history[history.length - 1];
  const gateRed = run.state.gate.filter((g) => !g.ok);
  // What stayed red at the very end: the gate's last verdict, or the review's
  // findings when that's what sent it back the last time.
  const final: FailItem[] =
    last?.kind === 'review' ? last.items : gateRed.length ? gateRed.map((g) => ({ name: g.name, detail: `$ ${g.command}\n${g.tail}` })) : (last?.items ?? []);
  const timesRed = (name: string) => history.filter((h) => h.items.some((i) => i.name === name)).length;
  const builder = run.team.find((a) => a.role === 'builder');
  const [accountId, setAccountId] = useState(builder?.accountId ?? accounts[0]?.id ?? '');
  const [terminalId, setTerminalId] = useState<string | null>(consoles.get(run.id) ?? null);
  const [opening, setOpening] = useState(false);

  const openConsole = async () => {
    if (!accountId) return;
    setOpening(true);
    try {
      const info = await invoke<TerminalInfo>('factory_run_console', { runId: run.id, accountId });
      consoles.set(run.id, info.id);
      setTerminalId(info.id);
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };
  const closeConsole = () => {
    consoles.delete(run.id);
    setTerminalId(null);
  };

  const what = last?.kind === 'review' ? `${final.length} ${final.length === 1 ? 'hallazgo siguió' : 'hallazgos siguieron'} sin resolver` : `${final.length} ${final.length === 1 ? 'check siguió' : 'checks siguieron'} en rojo hasta el final`;

  return (
    <details className={`${styles.cardFlat} ${styles.acc} ${styles.failPanel}`} open>
      <summary className={styles.accHead}>
        <span className={styles.sectionLabel}>POR QUÉ SE PARÓ {orderKey}</span>
        <span className={styles.hint}>
          {run.error ? `${run.error} · ` : ''}
          {final.length > 0 ? what : 'sin detalle guardado'}
        </span>
      </summary>

      {final.length > 0 && (
        <div className={styles.gateList}>
          {final.map((item) => {
            const n = timesRed(item.name);
            const always = history.length > 1 && n === history.length;
            return (
              <details key={item.name} className={styles.gateRow}>
                <summary className={styles.gateHead}>
                  <span className={styles.bad}>✗</span>
                  <span className={styles.lName}>{item.name}</span>
                  <span className={styles.spacer} />
                  {always ? (
                    <span className={styles.chipRed} title="El builder no lo arregló en ninguna ronda: mira si el fallo es del check o del entorno">
                      rojo en todas las rondas · quizá no es el código
                    </span>
                  ) : n > 0 ? (
                    <span className={styles.chipAmber}>
                      rojo en {n} de {history.length} rondas
                    </span>
                  ) : null}
                </summary>
                <pre className={`${styles.gateOut} ${styles.accBody}`}>{item.detail || '(sin salida)'}</pre>
              </details>
            );
          })}
        </div>
      )}

      {history.length > 0 && (
        <details className={`${styles.acc} ${styles.rounds}`}>
          <summary className={styles.accHead}>
            <span className={styles.lName}>Rondas · {history.length}</span>
            <span className={styles.hint}>qué mandó cada vuelta a Fabricación</span>
          </summary>
          <div className={styles.gateList}>
            {history.map((h) => (
              <details key={`${h.round}-${h.at}`} className={styles.gateRow}>
                <summary className={styles.gateHead}>
                  <span className={`${styles.mono} ${styles.dimTxt}`}>{h.round === 0 ? '1ª pasada' : `ronda ${h.round}`}</span>
                  <span className={styles.lName}>
                    {h.kind === 'gate' ? 'puerta' : 'revisión'} · {h.items.length} {h.kind === 'gate' ? 'en rojo' : h.items.length === 1 ? 'hallazgo' : 'hallazgos'}
                  </span>
                  <span className={`${styles.lSub} ${styles.gateCmd}`}>{h.items.map((i) => i.name).join(', ')}</span>
                </summary>
                <div className={styles.accBody}>
                  {h.items.map((i) => (
                    <div key={i.name} className={styles.roundItem}>
                      <div className={styles.lName}>{i.name}</div>
                      <pre className={styles.gateOut}>{i.detail}</pre>
                    </div>
                  ))}
                </div>
              </details>
            ))}
          </div>
        </details>
      )}

      <div className={styles.failActions}>
        {!terminalId && (
          <>
            <select className={styles.select} style={{ maxWidth: 200 }} value={accountId} onChange={(e) => setAccountId(e.target.value)}>
              {accounts.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
            </select>
            <button
              className={`${styles.btnPrimary} ${styles.sm}`}
              disabled={!accountId || opening}
              title="Claude en el worktree de la orden, con lo que quedó en rojo ya escrito"
              onClick={openConsole}
            >
              {opening ? 'Abriendo…' : 'Arreglar en una consola'}
            </button>
          </>
        )}
        {gateRed.length > 0 && (
          <button className={`${styles.btn} ${styles.sm}`} title="El builder recibe todos los rojos; las rondas empiezan de nuevo" onClick={() => onCheck('fix', null)}>
            Mandar a Fabricación
          </button>
        )}
        <span className={styles.spacer} />
        <button className={`${styles.btn} ${styles.sm}`} title="Vuelve a pasar la puerta donde quedó, con el equipo y el brain de ahora" onClick={() => onResolve('retry')}>
          {terminalId ? 'Listo: reintentar' : 'Reintentar donde quedó'}
        </button>
      </div>

      {terminalId && (
        <div className={styles.failConsole}>
          <div className={styles.failActions}>
            <span className={styles.hint}>
              Consola en el worktree de {orderKey} · también está en Terminales. Cuando haga commit, pulsa «Listo: reintentar».
            </span>
            <span className={styles.spacer} />
            <button className={`${styles.btnGhost} ${styles.sm}`} title="La oculta de aquí; sigue abierta en Terminales" onClick={closeConsole}>
              Ocultar consola
            </button>
          </div>
          <TerminalPane terminalId={terminalId} className={styles.terminal} />
        </div>
      )}
    </details>
  );
}
