import { useEffect, useState } from 'react';
import type { Account } from '../../types/account';
import { brainLabel, countdown, hhmm, STATION_NAME, type ActivityRow, type FactoryOrder, type LiveData, type Station, type StepRow } from '../../types/factory';
import { Timeline } from './Timeline';
import { Preparation } from './Preparation';
import styles from '../Factory.module.css';

type Big = 'ing' | 'fab' | 'cal' | 'ent';
const BIG: { id: Big; name: string; stations: Station[] }[] = [
  { id: 'ing', name: 'Ingeniería', stations: ['ing'] },
  { id: 'fab', name: 'Fabricación', stations: ['fab'] },
  { id: 'cal', name: 'Calidad', stations: ['test', 'rev', 'dec'] },
  { id: 'ent', name: 'Entrega', stations: ['ent'] },
];
const BIG_OF: Record<Station, Big> = { ing: 'ing', fab: 'fab', test: 'cal', rev: 'cal', dec: 'cal', ent: 'ent' };

const KIND: Record<ActivityRow['kind'], [string, string]> = {
  read: ['lee', styles.evRead],
  cmd: ['cmd', styles.evCmd],
  edit: ['escribe', styles.evEdit],
  brain: ['📖 brain', styles.evBrain],
  warn: ['aviso', styles.evWarn],
  gate: ['puerta', styles.evCmd],
  text: ['nota', ''],
};

const mins = (a: string, b: string | null) => Math.max(1, Math.round(((b ? new Date(b).getTime() : Date.now()) - new Date(a).getTime()) / 60_000));

/** A station's state, in the prototype's words. */
function stationState(steps: StepRow[], later: boolean): { text: string; cls: string } {
  if (!steps.length) return { text: 'pendiente', cls: '' };
  if (steps.some((s) => s.status === 'running')) return { text: `▶ ${steps.find((s) => s.status === 'running')!.label}`, cls: styles.stRun };
  if (steps.some((s) => s.status === 'wait')) return { text: '⏸ en espera · límite', cls: styles.stPaused };
  if (steps.some((s) => s.status === 'human' && !later)) return { text: '✋ te espera', cls: styles.stPaused };
  const last = steps[steps.length - 1];
  if (last.status === 'failed' && !later) return { text: `✗ ${last.note ?? 'falló'}`, cls: styles.stPaused };
  const minutes = steps.reduce((m, s) => m + (s.finishedAt ? mins(s.startedAt, s.finishedAt) : 0), 0);
  return { text: `✓ hecho · ${minutes} min`, cls: styles.stDone };
}

/**
 * «En vivo»: the order at work. The limit or ✋ banner, the four stations,
 * the timeline, what each agent did (every decision with its rule), the
 * quality layers and this order's decisions.
 */
/** What the loop is doing, as `factory_loop_status` says. */
export interface LoopStatus {
  state: 'stopped' | 'running' | 'paused';
  working: boolean;
  note: string | null;
  wait: 'quota' | 'merges' | 'blocked' | 'verify' | 'prepare' | null;
}

/** Before any order has run: what the loop is doing instead, in the open. */
function NotStarted({
  projectId,
  loop,
  failed,
  onStartAnyway,
  onRetry,
  onPlay,
  onGoStep,
}: {
  projectId: string;
  loop: LoopStatus | null;
  failed: FactoryOrder[];
  onStartAnyway: () => void;
  onRetry: (orderId: string) => void;
  onPlay: () => void;
  onGoStep: (step: number) => void;
}) {
  // ▶ was refused by the preparation: show it, with what to press.
  if (loop?.wait === 'prepare') {
    return <Preparation projectId={projectId} onGoStep={onGoStep} onPlay={onPlay} />;
  }
  if (loop?.wait === 'blocked') {
    return (
      <div className={styles.bannerRed}>
        <span>✗</span>
        <span>
          <b>{loop.note}</b>
        </span>
        <span className={styles.spacer} />
        {failed.map((o) => (
          <button key={o.id} className={`${styles.btn} ${styles.sm}`} onClick={() => onRetry(o.id)}>
            Reintentar {o.key}
          </button>
        ))}
      </div>
    );
  }
  if (loop && loop.state !== 'stopped' && loop.note) {
    return (
      <div className={styles.bannerAmber}>
        <span style={{ fontSize: 15 }}>⏸</span>
        <span>
          <b>{loop.note}</b>
          {loop.wait === 'quota' && <span className={styles.hint}> · vuelve a mirar cada 2 minutos; si llega al límite real, la orden espera sola al reset.</span>}
        </span>
        <span className={styles.spacer} />
        {loop.wait === 'quota' && (
          <button className={`${styles.btn} ${styles.sm}`} onClick={onStartAnyway}>
            Empezar igualmente
          </button>
        )}
      </div>
    );
  }
  if (loop && loop.state !== 'stopped') {
    return (
      <span className={styles.hint}>
        <span className={styles.spin} />
        arrancando con la primera orden de la cola…
      </span>
    );
  }
  if (loop?.note) {
    return (
      <div className={styles.bannerRed}>
        <span>✗</span>
        <span>
          <b>{loop.note}</b>
        </span>
        <span className={styles.spacer} />
        <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={onPlay}>
          ▶ Preparar y seguir
        </button>
      </div>
    );
  }
  return <span className={styles.hint}>Todavía no ha pasado ninguna orden por la línea. Pulsa ▶ Play para empezar con la primera de la cola.</span>;
}

export function LiveView({
  live,
  loop,
  accounts,
  onResolve,
  onStartAnyway,
  failed,
  onRetry,
  onPlay,
  onGoStep,
  projectId,
}: {
  live: LiveData;
  loop: LoopStatus | null;
  accounts: Account[];
  onResolve: (action: string, note?: string, accountId?: string) => void;
  onStartAnyway: () => void;
  /** Failed or stopped orders, to retry from the «blocked» banner. */
  failed: FactoryOrder[];
  onRetry: (orderId: string) => void;
  /** ▶ — which prepares the project first. */
  onPlay: () => void;
  /** Opens the wizard at a step, to fix something there. */
  onGoStep: (step: number) => void;
  projectId: string;
}) {
  const run = live.run;
  const [picked, setPicked] = useState<Big | null>(null);
  const [note, setNote] = useState('');
  const [other, setOther] = useState('');
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);

  if (!run || !live.order) {
    return <NotStarted projectId={projectId} loop={loop} failed={failed} onStartAnyway={onStartAnyway} onRetry={onRetry} onPlay={onPlay} onGoStep={onGoStep} />;
  }

  const name = (id: string | null | undefined) => accounts.find((a) => a.id === id)?.name ?? '—';
  const team = run.team.filter((a) => a.active);
  const current = live.steps[live.steps.length - 1];
  const auto: Big = current ? BIG_OF[current.station] : 'ing';
  const sel = picked ?? auto;
  const reached = new Set(live.steps.map((s) => BIG_OF[s.station]));
  const order = live.order;
  const stuckAgent = team.find((a) => a.id === [...live.steps].reverse().find((s) => s.agentId)?.agentId);

  const stationMeta = (b: Big) => {
    const roles: string[] = { ing: ['spec', 'critic'], fab: ['builder'], cal: [], ent: [] }[b];
    if (b === 'cal') return `testers ×${team.filter((a) => a.role === 'tester').length} · revisores ×${team.filter((a) => a.role.startsWith('reviewer')).length}`;
    if (b === 'ent') return run.mrUrl ? 'MR abierta' : run.base;
    const a = team.find((x) => roles.includes(x.role));
    return a ? `${name(a.accountId)} · ${a.model}` : '';
  };
  const stationBrain = (b: Big) => {
    const roles: string[] = { ing: ['spec'], fab: ['builder'], cal: ['tester'], ent: ['approver'] }[b];
    const a = team.find((x) => roles.includes(x.role));
    return b === 'cal' ? 'reglas · 1 sección c/u' : a ? brainLabel(a.sections) : '';
  };

  const feed = live.activity.filter((a) => BIG_OF[a.station] === sel);
  const resumeAt = run.resumeAt ? new Date(run.resumeAt).getTime() : null;
  const testers = team.filter((a) => a.role === 'tester');
  const reviewers = team.filter((a) => a.role.startsWith('reviewer'));
  const deciders = team.filter((a) => a.role === 'verifier' || a.role === 'approver');
  const layerState = (stations: Station[], label?: string) => {
    const steps = live.steps.filter((s) => stations.includes(s.station) && (!label || s.label === label));
    if (!steps.length) return 'en cola';
    if (steps.some((s) => s.status === 'running')) return 'en curso';
    const last = steps[steps.length - 1];
    return last.status === 'failed' ? `✗ ${last.note ?? ''}` : last.status === 'done' ? `✓ ${last.note ?? ''}`.trim() : last.status;
  };

  return (
    <>
      {run.status === 'waiting_limit' && resumeAt && (
        <div className={styles.bannerAmber}>
          <span style={{ fontSize: 15 }}>⏸</span>
          <span>
            <b>Límite de uso de la cuenta «{name(stuckAgent?.accountId)}».</b> {order.key} espera en {STATION_NAME[current?.station ?? 'fab']}, en la misma conversación; no
            cuenta como fallo.
          </span>
          <span className={styles.spacer} />
          <span>
            sigue sola a las <b className={styles.mono}>{hhmm(run.resumeAt!)}</b> · <span className={styles.countdown}>{countdown((resumeAt - now) / 1000)}</span>
          </span>
          <select className={styles.select} style={{ maxWidth: 180 }} value={other} onChange={(e) => setOther(e.target.value)}>
            <option value="">otra cuenta…</option>
            {accounts
              .filter((a) => a.id !== stuckAgent?.accountId)
              .map((a) => (
                <option key={a.id} value={a.id}>
                  {a.name}
                </option>
              ))}
          </select>
          <button className={`${styles.btn} ${styles.sm}`} disabled={!other} onClick={() => onResolve('other_account', undefined, other)}>
            Usar otra cuenta
          </button>
        </div>
      )}

      {run.status === 'waiting_human' && (
        <div className={styles.bannerAcc}>
          <span style={{ fontSize: 15 }}>✋</span>
          <span style={{ flex: '1 1 320px' }}>
            <b>{run.state.human}</b>
            {run.state.humanKind === 'spec_review' && run.state.spec && <span className={styles.hint}> · {String(run.state.spec.summary ?? '')}</span>}
          </span>
          {(run.state.humanKind === 'spec_review' || run.state.humanKind === 'diff' || run.state.humanKind === 'approver') && (
            <input className={styles.input} style={{ maxWidth: 260 }} placeholder="nota (si la devuelves)" value={note} onChange={(e) => setNote(e.target.value)} />
          )}
          {run.state.humanKind === 'spec_review' && (
            <>
              <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={() => onResolve('continue')}>
                Aprobar spec y fabricar
              </button>
              <button className={`${styles.btn} ${styles.sm}`} onClick={() => onResolve('reject_spec', note)}>
                Rehacer la spec
              </button>
            </>
          )}
          {(run.state.humanKind === 'diff' || run.state.humanKind === 'approver') && (
            <>
              <span className={`${styles.hint} ${styles.mono}`}>{run.worktree}</span>
              <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={() => onResolve('continue')}>
                Entregar
              </button>
              <button className={`${styles.btn} ${styles.sm}`} onClick={() => onResolve('send_back', note)}>
                Devolver a Fabricación
              </button>
            </>
          )}
          {run.state.humanKind === 'credits' && (
            <>
              <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={() => onResolve('retry')}>
                Reintentar
              </button>
              <select className={styles.select} style={{ maxWidth: 180 }} value={other} onChange={(e) => setOther(e.target.value)}>
                <option value="">otra cuenta…</option>
                {accounts.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name}
                  </option>
                ))}
              </select>
              <button className={`${styles.btn} ${styles.sm}`} disabled={!other} onClick={() => onResolve('other_account', undefined, other)}>
                Usar otra cuenta
              </button>
            </>
          )}
          {run.state.humanKind === 'merge' && (
            <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={() => onResolve('retry')}>
              Reintentar la entrega
            </button>
          )}
        </div>
      )}

      {run.status === 'failed' && (
        <div className={styles.bannerRed}>
          <span>✗</span>
          <span>
            <b>{order.key} falló.</b> {run.error}
          </span>
          <span className={styles.spacer} />
          <button className={`${styles.btn} ${styles.sm}`} onClick={() => onResolve('retry')}>
            Reintentar donde quedó
          </button>
        </div>
      )}

      <div className={styles.order}>
        <span className={styles.orderKey}>{order.key}</span>
        <span className={styles.orderTitle}>{order.title}</span>
        <span className={styles.chip}>rama {run.branch}</span>
        <span className={styles.chip}>desde {run.base}</span>
        {run.round > 0 && <span className={styles.chipAmber}>ronda {run.round}</span>}
        <span className={styles.spacer} />
        <span className={styles.num}>
          ${run.costUsd.toFixed(2)} hasta ahora · {mins(run.startedAt, run.finishedAt)} min
        </span>
      </div>

      <div className={styles.line}>
        {BIG.map((b, i) => {
          const steps = live.steps.filter((s) => b.stations.includes(s.station));
          const later = BIG.slice(i + 1).some((x) => reached.has(x.id));
          const st = stationState(steps, later);
          return (
            <button key={b.id} className={`${styles.station} ${st.cls} ${sel === b.id ? styles.stSel : ''}`} onClick={() => setPicked(b.id)}>
              <span className={styles.stName}>{b.name}</span>
              <span className={styles.stState}>{st.text}</span>
              <span className={styles.stBrain}>brain: {stationBrain(b.id)}</span>
              <span className={styles.stMeta}>{stationMeta(b.id)}</span>
            </button>
          );
        })}
      </div>

      <Timeline live={live} onPickStation={(s) => setPicked(BIG_OF[s])} />

      <div className={styles.split}>
        <div className={styles.cardFlat}>
          <div className={styles.cardHead}>
            <span className={styles.sectionLabel}>ACTIVIDAD · {BIG.find((b) => b.id === sel)?.name.toUpperCase()}</span>
            <span className={styles.spacer} />
            <span className={styles.chipAcc}>cada decisión cita su regla</span>
          </div>
          <div className={styles.feed}>
            {feed.length === 0 && <span className={`${styles.hint} ${styles.cardPadCol}`}>Esta estación todavía no ha hecho nada en esta orden.</span>}
            {feed.map((a) => {
              const [label, cls] = KIND[a.kind] ?? ['', ''];
              return (
                <div key={a.id} className={styles.ev}>
                  <span className={styles.evAt}>{hhmm(a.at)}</span>
                  <span className={`${styles.evKind} ${cls}`}>{label}</span>
                  <span className={styles.evTxt}>
                    {a.ruleId && <span className={styles.rule}>{a.ruleId}</span>}
                    {a.text}
                  </span>
                </div>
              );
            })}
          </div>
        </div>
        <div className={styles.col} style={{ gap: 16 }}>
          <div className={styles.cardFlat}>
            <div className={styles.cardHead}>
              <span className={styles.sectionLabel}>CALIDAD · EN CAPAS</span>
            </div>
            <div className={styles.layers}>
              {[
                [1, `Testers ×${testers.length}`, testers.map((a) => `${a.short} · ${a.model}`).join('  ·  '), layerState(['test'])],
                [2, '¿Tests en verde? · obligatorio', `${testers.map((a) => a.command).join(' · ')} · sin IA`, layerState(['test'], '¿Suite en verde?')],
                [3, `Revisores ×${reviewers.length} · en paralelo`, reviewers.map((a) => `${a.short} · ${a.model}`).join('  ·  '), layerState(['rev'])],
                [4, 'Verificador → Aprobador', deciders.map((a) => `${a.short} · ${a.model}`).join('  ·  '), layerState(['dec'])],
              ].map(([n, title, sub, state]) => (
                <div key={n as number} className={styles.layer}>
                  <span className={`${styles.dimTxt} ${styles.mono}`}>{n}</span>
                  <div>
                    <div className={styles.lName}>{title}</div>
                    <div className={styles.lSub}>{sub}</div>
                  </div>
                  <span className={styles.lSub}>{state}</span>
                </div>
              ))}
            </div>
          </div>
          <div className={styles.cardFlat}>
            <div className={styles.cardHead}>
              <span className={styles.sectionLabel}>DECISIONES DE ESTA ORDEN</span>
            </div>
            {live.decisions.length === 0 && <span className={`${styles.hint} ${styles.cardPadCol}`}>Ninguna todavía.</span>}
            {live.decisions.map((d, i) => (
              <div key={i} className={styles.dec}>
                <span>
                  <span className={d.ruleId ? styles.rule : styles.ruleGap}>{d.ruleId ?? 'sin regla'}</span>
                </span>
                <span>
                  {d.text}
                  {!d.ruleId && (
                    <>
                      {' '}
                      — va a la <b>bandeja de enmiendas</b>.
                    </>
                  )}
                </span>
              </div>
            ))}
          </div>
          {run.state.warnings.length > 0 && (
            <div className={styles.cardFlat}>
              <div className={styles.cardHead}>
                <span className={styles.sectionLabel}>AVISOS</span>
              </div>
              {run.state.warnings.map((w) => (
                <div key={w} className={styles.dec} style={{ gridTemplateColumns: '1fr' }}>
                  <span className={styles.warn}>⚠ {w}</span>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </>
  );
}
