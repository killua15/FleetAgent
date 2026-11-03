import { useCallback, useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import type { Account } from '../../types/account';
import { providerShort, providerWhere, type Delivery, type FactoryOrder, type FactoryProject, type LiveData, type SealedBrain } from '../../types/factory';
import { BrainView } from './BrainView';
import { DeliveriesView } from './DeliveriesView';
import { LiveView, type LoopStatus } from './LiveView';
import type { CheckAction } from './GatePanel';
import { QueueList } from './QueueList';
import { TeamEditor } from './TeamEditor';
import styles from '../Factory.module.css';

type Tab = 'live' | 'queue' | 'brain' | 'team' | 'out';


/** What the line is doing, in one line, as the prototype's status bar. */
function statusLine(loop: LoopStatus | null, live: LiveData | null, orders: FactoryOrder[]): string {
  const run = live?.run;
  const key = live?.order?.key;
  const position = key ? `orden ${orders.findIndex((o) => o.key === key) + 1} de ${orders.length}` : '';
  if (!loop) return '';
  if (run?.status === 'waiting_limit' && run.resumeAt) return `en espera por límite de uso · sigue sola a las ${new Date(run.resumeAt).toTimeString().slice(0, 5)}`;
  if (run?.status === 'waiting_human') return `✋ ${key} te espera`;
  if (loop.state === 'paused') return `en pausa: termina ${key ?? 'la orden en curso'} y no toma más`;
  if (loop.state === 'stopped') return loop.note ?? (run?.status === 'stopped' ? `parada · Reintentar retoma ${key} donde quedó` : 'parada');
  if (run?.status === 'running') return `trabajando en ${key} · ${position}`;
  return loop.note ?? 'trabajando';
}

/**
 * The line at work: ▶ ⏸ ⏹, and the five tabs — En vivo, Cola, Brain, Equipo,
 * Entregas. It refreshes on every `factory-updated` the backend emits.
 */
export function Ops({ project, onReconfigure, onChanged }: { project: FactoryProject; onReconfigure: (step?: number) => void; onChanged: () => void }) {
  const [tab, setTab] = useState<Tab>('live');
  const [live, setLive] = useState<LiveData | null>(null);
  const [loop, setLoop] = useState<LoopStatus | null>(null);
  const [orders, setOrders] = useState<FactoryOrder[]>([]);
  const [deliveries, setDeliveries] = useState<Delivery[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [brainCount, setBrainCount] = useState(0);
  const [teamCount, setTeamCount] = useState(0);
  const pid = project.projectId;

  const refresh = useCallback(() => {
    invoke<LiveData>('factory_live', { projectId: pid }).then(setLive).catch(() => undefined);
    invoke<LoopStatus>('factory_loop_status', { projectId: pid }).then(setLoop).catch(() => undefined);
    invoke<FactoryOrder[]>('factory_orders_list', { projectId: pid }).then(setOrders).catch(() => undefined);
    invoke<Delivery[]>('factory_deliveries', { projectId: pid }).then(setDeliveries).catch(() => undefined);
  }, [pid]);

  // Events arrive in bursts while an agent works; one read per burst.
  const pending = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => {
    refresh();
    invoke<Account[]>('list_accounts').then(setAccounts).catch(() => setAccounts([]));
    invoke<SealedBrain | null>('factory_brain_get', { projectId: pid, version: null })
      .then((b) => setBrainCount(b?.rules.length ?? 0))
      .catch(() => undefined);
    const unlisten = listen<string>('factory-updated', (e) => {
      if (e.payload !== pid || pending.current) return;
      pending.current = setTimeout(() => {
        pending.current = null;
        refresh();
      }, 400);
    });
    const timer = setInterval(refresh, 10_000);
    return () => {
      unlisten.then((f) => f());
      clearInterval(timer);
      if (pending.current) clearTimeout(pending.current);
    };
  }, [pid, refresh]);

  const control = async (cmd: 'factory_play' | 'factory_pause' | 'factory_stop', said: string) => {
    try {
      setLoop(await invoke<LoopStatus>(cmd, { projectId: pid }));
      toastSuccess(said);
      refresh();
      onChanged();
    } catch (e) {
      toastError(String(e));
    }
  };

  const resolve = async (action: string, note?: string, accountId?: string) => {
    if (!live?.run) return;
    try {
      await invoke('factory_run_resolve', { runId: live.run.id, action, note: note ?? null, accountId: accountId ?? null });
      refresh();
    } catch (e) {
      toastError(String(e));
    }
  };

  const check = async (action: CheckAction, command: string | null, note?: string) => {
    if (!live?.run) return;
    try {
      await invoke('factory_run_check', { runId: live.run.id, action, check: command, note: note?.trim() ? note : null });
      toastSuccess(
        action === 'fix'
          ? 'Vuelve a Fabricación con la salida del check'
          : action === 'broken'
            ? 'A la bandeja del Brain: no corre hasta que lo arregles, selles y reintentes'
            : 'Ignorado en esta orden: queda en los avisos de la entrega',
      );
      refresh();
    } catch (e) {
      toastError(String(e));
    }
  };

  const retry = async (orderId: string) => {
    try {
      await invoke('factory_order_retry', { orderId });
      toastSuccess('Vuelve a la cola: pulsa ▶ para seguir');
      refresh();
    } catch (e) {
      toastError(String(e));
    }
  };

  const startAnyway = async () => {
    try {
      setLoop(await invoke<LoopStatus>('factory_start_anyway', { projectId: pid }));
      refresh();
    } catch (e) {
      toastError(String(e));
    }
  };

  const reorder = async (ids: string[]) => {
    try {
      setOrders(await invoke<FactoryOrder[]>('factory_orders_reorder', { projectId: pid, ids }));
    } catch (e) {
      toastError(String(e));
    }
  };

  const exportLater = async () => {
    try {
      const n = await invoke<number>('factory_orders_export', { projectId: pid });
      toastSuccess(`${n} órdenes exportadas a ${providerWhere(project)}`);
      refresh();
    } catch (e) {
      toastError(String(e));
    }
  };

  const running = loop?.state === 'running';
  const queued = orders.filter((o) => o.status !== 'merged' && o.status !== 'failed').length;
  const local = project.providerKind === 'none';

  return (
    <section className={styles.setup}>
      <div className={styles.row} style={{ padding: '10px 24px 0' }}>
        <span className={styles.statusLine} style={{ padding: 0 }}>
          {statusLine(loop, live, orders)}
        </span>
        <span className={styles.spacer} />
        <div className={styles.controls}>
          <button className={styles.btnPrimary} disabled={running} onClick={() => control('factory_play', '▶ Toma la siguiente orden de la cola')}>
            ▶ Play
          </button>
          <button className={styles.btn} disabled={!running} onClick={() => control('factory_pause', '⏸ Termina la orden en curso y no toma más')}>
            ⏸ Pausa
          </button>
          <button className={styles.btnDanger} disabled={loop?.state === 'stopped' && !loop.working} onClick={() => control('factory_stop', '⏹ Parada: ▶ la retoma donde quedó')}>
            ⏹ Stop
          </button>
        </div>
        <button className={`${styles.btnGhost} ${styles.sm}`} title="Ver la configuración desde el principio" onClick={() => onReconfigure()}>
          ↺ Reconfigurar
        </button>
      </div>
      <nav className={styles.tabs}>
        {(
          [
            ['live', 'En vivo', null],
            ['queue', 'Cola', queued],
            ['brain', 'Brain', brainCount],
            ['team', 'Equipo', teamCount],
            ['out', 'Entregas', deliveries.length],
          ] as [Tab, string, number | null][]
        ).map(([id, label, n]) => (
          <button key={id} className={tab === id ? styles.tabOn : styles.tab} onClick={() => setTab(id)}>
            {label} {n !== null && <span className={styles.count}>{n}</span>}
          </button>
        ))}
      </nav>
      <div className={styles.opsBody}>
        {tab === 'live' && (live ? <LiveView
              live={live}
              loop={loop}
              accounts={accounts}
              onResolve={resolve}
              onCheck={check}
              onStartAnyway={startAnyway}
              failed={orders.filter((o) => o.status === 'failed' || o.status === 'stopped')}
              onRetry={retry}
              onPlay={() => control('factory_play', '▶ Proyecto preparado: toma la siguiente orden')}
              onGoStep={(n) => onReconfigure(n)}
              projectId={pid}
            /> : null)}
        {tab === 'queue' && (
          <>
            {local && (
              <div className={styles.banner}>
                <span>📦</span>
                <span>
                  <b>Sin proveedor:</b> las órdenes viven en Hangar. Conecta ClickUp o GitHub cuando quieras y se exportan las que nadie empezó.
                </span>
                <span className={styles.spacer} />
                <button className={`${styles.btn} ${styles.sm}`} onClick={() => onReconfigure(1)}>
                  Conectar y exportar
                </button>
              </div>
            )}
            {!local && orders.some((o) => !o.extId && o.status === 'todo') && (
              <div className={styles.banner}>
                <span>Hay órdenes que viven solo en Hangar.</span>
                <span className={styles.spacer} />
                <button className={`${styles.btn} ${styles.sm}`} onClick={exportLater}>
                  Exportarlas a {providerShort(project)}
                </button>
              </div>
            )}
            <span className={styles.hint}>
              Solo las tareas de <b>este proyecto</b> ({providerWhere(project)}). ▶ las toma de una en una, <b>en este orden</b>: arrastra para reordenar. Las bloqueadas
              esperan a sus dependencias.
            </span>
            <QueueList orders={orders} providerShort={providerShort(project)} onReorder={reorder} onRetry={retry} />
          </>
        )}
        {tab === 'brain' && (
          <BrainView
            project={project}
            onSealed={() => {
              onChanged();
              invoke<SealedBrain | null>('factory_brain_get', { projectId: pid, version: null })
                .then((b) => setBrainCount(b?.rules.length ?? 0))
                .catch(() => undefined);
            }}
          />
        )}
        {tab === 'team' && (
          <>
            <span className={styles.hint}>Los cambios valen para las órdenes que empiecen a partir de ahora; la que está en curso sigue con su equipo.</span>
            <TeamEditor projectId={pid} onChange={(v) => setTeamCount(v.agents.filter((a) => a.active).length)} />
          </>
        )}
        {tab === 'out' && <DeliveriesView deliveries={deliveries} />}
      </div>
    </section>
  );
}
