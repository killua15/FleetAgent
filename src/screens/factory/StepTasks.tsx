import { useCallback, useEffect, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import { TerminalPane } from '../../components/TerminalPane';
import type { TerminalInfo } from '../../types/terminal';
import { providerShort, providerWhere, type FactoryOrder, type FactoryProject, type TrdPlan } from '../../types/factory';
import { QueueList } from './QueueList';
import { StepFoot } from './Wizard';
import styles from '../Factory.module.css';

type Source = 'prov' | 'trd';
type TrdInput = 'paste' | 'file' | 'link';

/**
 * Step 4 · Tareas: where the orders come from. A — the project's own list or
 * repo (or, without a provider, Hangar's local backlog); B — a TRD the planner
 * breaks down, which you always review before anything is created.
 */
export function StepTasks({ project, onBack, onNext, onToProvider }: { project: FactoryProject; onBack: () => void; onNext: () => void; onToProvider: () => void }) {
  const local = project.providerKind === 'none';
  const [source, setSource] = useState<Source>('trd');
  const [orders, setOrders] = useState<FactoryOrder[]>([]);
  const [trd, setTrd] = useState('');
  const [input, setInput] = useState<TrdInput>('paste');
  const [linked, setLinked] = useState<string | null>(null);
  const [analyzing, setAnalyzing] = useState(false);
  const [plan, setPlan] = useState<TrdPlan | null>(null);
  const [approving, setApproving] = useState(false);
  const [created, setCreated] = useState<FactoryOrder[] | null>(null);
  const [refineTerminal, setRefineTerminal] = useState<string | null>(null);
  const [objection, setObjection] = useState('');
  const [syncing, setSyncing] = useState(false);
  const [manualTitle, setManualTitle] = useState('');
  const [manualCriteria, setManualCriteria] = useState('');

  const load = useCallback(
    () =>
      invoke<FactoryOrder[]>('factory_orders_list', { projectId: project.projectId })
        .then(setOrders)
        .catch((e) => toastError(String(e))),
    [project.projectId],
  );
  useEffect(() => {
    load();
  }, [load]);

  const readLinked = async (path: string) => invoke<string>('factory_read_markdown', { path });

  const pickTrd = async (mode: TrdInput) => {
    const path = await openDialog({ multiple: false, title: 'Elegir el TRD', filters: [{ name: 'Markdown o texto', extensions: ['md', 'markdown', 'txt'] }] });
    if (typeof path !== 'string') return;
    try {
      const text = await readLinked(path);
      setTrd(text);
      setLinked(mode === 'link' ? path : null);
      setInput(mode);
    } catch (e) {
      toastError(String(e));
    }
  };

  const analyze = async (override?: string) => {
    if (!project.brainVersion) {
      toastError('Primero sella el brain (paso 3): el planificador decide con él');
      return;
    }
    setAnalyzing(true);
    setPlan(null);
    try {
      // A linked .md is read again on every analysis: it's the file that counts.
      const text = override ?? (linked ? await readLinked(linked) : trd);
      if (linked) setTrd(text);
      setPlan(await invoke<TrdPlan>('factory_plan_trd', { projectId: project.projectId, trd: text }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setAnalyzing(false);
    }
  };

  const approve = async () => {
    if (!plan) return;
    setApproving(true);
    try {
      const made = await invoke<FactoryOrder[]>('factory_plan_approve', { projectId: project.projectId, plan });
      setCreated(made);
      setPlan(null);
      await load();
      toastSuccess(local ? `Guardadas ${made[0]?.key}…${made[made.length - 1]?.key} en el backlog local` : `Creadas ${made.length} órdenes en ${providerWhere(project)}`);
    } catch (e) {
      toastError(String(e));
    } finally {
      setApproving(false);
    }
  };

  const refine = async () => {
    try {
      const info = await invoke<TerminalInfo>('factory_trd_refine', { projectId: project.projectId, trd, objection });
      setRefineTerminal(info.id);
    } catch (e) {
      toastError(String(e));
    }
  };

  const reanalyze = async () => {
    const text = await invoke<string | null>('factory_trd_read', { projectId: project.projectId });
    if (text) setTrd(text);
    setLinked(null);
    setInput('paste');
    setRefineTerminal(null);
    analyze(text ?? trd);
  };

  const sync = async () => {
    setSyncing(true);
    try {
      const added = await invoke<number>('factory_orders_sync', { projectId: project.projectId });
      await load();
      toastSuccess(added ? `${added} tareas nuevas en la cola` : 'No hay tareas nuevas asignadas a ti en esa lista');
    } catch (e) {
      toastError(String(e));
    } finally {
      setSyncing(false);
    }
  };

  const addManual = async () => {
    try {
      setOrders(
        await invoke<FactoryOrder[]>('factory_order_add', {
          projectId: project.projectId,
          title: manualTitle,
          criteria: manualCriteria.split('\n'),
          deps: [],
        }),
      );
      setManualTitle('');
      setManualCriteria('');
    } catch (e) {
      toastError(String(e));
    }
  };

  const exportLater = async () => {
    try {
      const n = await invoke<number>('factory_orders_export', { projectId: project.projectId });
      await load();
      toastSuccess(`${n} órdenes exportadas a ${providerWhere(project)}`);
    } catch (e) {
      toastError(String(e));
    }
  };

  const reorder = async (ids: string[]) => {
    try {
      setOrders(await invoke<FactoryOrder[]>('factory_orders_reorder', { projectId: project.projectId, ids }));
    } catch (e) {
      toastError(String(e));
    }
  };

  const unexported = orders.filter((o) => !o.extId && o.status === 'todo').length;
  const pending = orders.filter((o) => o.status !== 'merged').length;

  return (
    <>
      <div className={styles.stepBody}>
        <span className={styles.lead}>Las tareas: ¿de dónde salen?</span>
        <div className={styles.sourcePick}>
          <button className={source === 'prov' ? styles.sourceOn : styles.source} onClick={() => setSource('prov')}>
            <span className={styles.sectionLabel}>{local ? 'A · SIN PROVEEDOR' : 'A · PROVEEDOR'}</span>
            <b>{local ? 'Backlog local en Hangar' : providerShort(project)}</b>
            <span className={styles.hint}>
              {local
                ? 'Escribe las tareas a mano o sácalas de un TRD: se guardan en Hangar. Puedes conectar un proveedor más adelante.'
                : `Las tareas que ya existan en ${providerWhere(project)}. Solo de este proyecto.`}
            </span>
          </button>
          <button className={source === 'trd' ? styles.sourceOn : styles.source} onClick={() => setSource('trd')}>
            <span className={styles.sectionLabel}>B · DESDE UN TRD</span>
            <b>El planificador saca las órdenes</b>
            <span className={styles.hint}>
              Lee el TRD, el brain {project.brainVersion ? `v${project.brainVersion}` : '(sin sellar)'} y el repo. Tú revisas el desglose; aprobado, se guarda en{' '}
              {providerWhere(project)}.
            </span>
          </button>
        </div>

        {source === 'prov' &&
          (local ? (
            <div className={styles.cardPadCol} style={{ border: '1px solid var(--line)', borderRadius: 10, background: 'var(--panel)' }}>
              <span className={styles.sectionLabel}>UNA ORDEN A MANO</span>
              <input className={styles.input} placeholder="título · p. ej. «Reservar una cita»" value={manualTitle} onChange={(e) => setManualTitle(e.target.value)} />
              <textarea
                className={styles.textarea}
                style={{ minHeight: 80 }}
                placeholder="criterios de aceptación, uno por línea: cada uno será un test"
                value={manualCriteria}
                onChange={(e) => setManualCriteria(e.target.value)}
              />
              <div className={styles.row}>
                <button className={styles.btnPrimary} disabled={!manualTitle.trim()} onClick={addManual}>
                  + Añadir a la cola
                </button>
                <span className={styles.spacer} />
                <button className={`${styles.btn} ${styles.sm}`} onClick={onToProvider}>
                  Conectar un proveedor
                </button>
              </div>
            </div>
          ) : (
            <div className={styles.row}>
              <button className={styles.btnPrimary} disabled={syncing} onClick={sync}>
                {syncing ? 'leyendo…' : `Traer las tareas de ${providerShort(project)}`}
              </button>
              <span className={styles.hint}>Solo las abiertas y asignadas a ti en {providerWhere(project)}: nada de otros proyectos.</span>
            </div>
          ))}

        {source === 'trd' && (
          <div className={styles.cardPadCol} style={{ border: '1px solid var(--line)', borderRadius: 10, background: 'var(--panel)' }}>
            <div className={styles.row}>
              <span className={styles.sectionLabel}>TRD</span>
              <span className={styles.spacer} />
              <div className={styles.seg}>
                <button aria-pressed={input === 'paste'} onClick={() => (setInput('paste'), setLinked(null))}>
                  Pegar texto
                </button>
                <button aria-pressed={input === 'file'} onClick={() => pickTrd('file')}>
                  Importar archivo
                </button>
                <button aria-pressed={input === 'link'} onClick={() => pickTrd('link')}>
                  Enlazar .md
                </button>
              </div>
            </div>
            {linked && <span className={`${styles.hint} ${styles.mono}`}>enlazado: {linked} · se relee al analizar</span>}
            <textarea
              className={styles.textarea}
              value={trd}
              readOnly={!!linked}
              placeholder={'## 1 · Reservar una cita\nPOST /citas · cliente, mascota, veterinario, franja de 30 min. Sin solapes.\n\n## 2 · …'}
              onChange={(e) => setTrd(e.target.value)}
            />
            <div className={styles.row}>
              <span className={styles.hint}>
                {analyzing ? (
                  <>
                    <span className={styles.spin} />
                    leyendo el TRD, el brain v{project.brainVersion} y el repo…
                  </>
                ) : plan ? (
                  'Desglose listo: revísalo abajo.'
                ) : project.brainVersion ? (
                  `Listo para analizar con el brain v${project.brainVersion}.`
                ) : (
                  'Sella antes el brain: el planificador decide con él.'
                )}
              </span>
              <span className={styles.spacer} />
              {analyzing ? (
                <button className={styles.btnDanger} onClick={() => invoke('factory_plan_cancel', { projectId: project.projectId })}>
                  ⏹ Cancelar el análisis
                </button>
              ) : (
                <button className={styles.btnPrimary} disabled={!trd.trim() || !project.brainVersion} onClick={() => analyze()}>
                  🧩 Analizar y sacar tareas
                </button>
              )}
            </div>
          </div>
        )}

        {plan && (
          <div className={styles.cardFlat}>
            <div className={styles.cardHead}>
              <span className={styles.sectionLabel}>✋ REVISAS EL DESGLOSE · OBLIGATORIO</span>
              <span className={styles.spacer} />
              <span className={styles.chip}>{plan.orders.length} órdenes</span>
            </div>
            <div className={styles.grid2} style={{ padding: '0 16px 16px' }}>
              <div>
                {plan.orders.map((o, i) => (
                  <div key={i} className={styles.draft}>
                    <span className={styles.qKey}>#{i + 1}</span>
                    <span>
                      <span className={styles.qTitle}>{o.title}</span>
                      {o.criteria.length > 0 && <span className={styles.aRole}> · {o.criteria.length} criterios</span>}
                    </span>
                    <span className={styles.qDeps}>{o.deps.length ? `⤷ ${o.deps.map((d) => `#${d}`).join(' · ')}` : o.trdPart ? `TRD ${o.trdPart}` : ''}</span>
                  </div>
                ))}
              </div>
              <div>
                <table className={styles.cov}>
                  <thead>
                    <tr>
                      <th>Parte del TRD</th>
                      <th>Órdenes</th>
                      <th>Reglas que aplican</th>
                    </tr>
                  </thead>
                  <tbody>
                    {plan.coverage.map((c) => (
                      <tr key={c.part}>
                        <td>{c.part}</td>
                        <td className={styles.mono}>{c.orders.map((n) => `#${n}`).join(' · ')}</td>
                        <td className={`${styles.mono} ${styles.accTxt}`}>{c.rules.join(' · ')}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
                {plan.risks.map((r) => (
                  <p key={r} className={styles.hint} style={{ margin: '12px 0 0' }}>
                    Riesgo: <span className={styles.warn}>{r}</span>
                  </p>
                ))}
                <p className={styles.hint} style={{ margin: '6px 0 0' }}>
                  Sin cubrir: {plan.uncovered.length ? <span className={styles.bad}>{plan.uncovered.join(' · ')}</span> : 'nada'}.
                </p>
              </div>
            </div>
            <div className={styles.cardActions}>
              <button className={styles.btnPrimary} disabled={approving} onClick={approve}>
                {approving ? 'creando…' : local ? 'Aprobar → guardar en local (Hangar)' : `Aprobar → crear en ${providerShort(project)}`}
              </button>
              <button className={styles.btn} onClick={refine}>
                No me convence → afinar el TRD en una terminal
              </button>
              <button className={styles.btnGhost} onClick={() => setPlan(null)}>
                Descartar
              </button>
            </div>
            {!refineTerminal && (
              <div className={styles.cardPad0}>
                <input
                  className={styles.input}
                  placeholder="(opcional) qué no te convence, para la terminal"
                  value={objection}
                  onChange={(e) => setObjection(e.target.value)}
                />
              </div>
            )}
          </div>
        )}

        {refineTerminal && (
          <div className={`${styles.cardFlat} ${styles.chat}`}>
            <div className={styles.cardHead}>
              <span className={styles.sectionLabel}>AFINAR EL TRD · terminal</span>
              <span className={styles.spacer} />
              <span className={`${styles.hint} ${styles.mono}`}>.hangar/trd/TRD.md</span>
            </div>
            <TerminalPane terminalId={refineTerminal} className={styles.terminal} />
            <div className={styles.chatFoot}>
              <span className={styles.hint}>Cuando la IA haya editado el TRD contigo, vuelve a analizarlo y a revisarlo.</span>
              <span className={styles.spacer} />
              <button className={styles.btnPrimary} onClick={reanalyze}>
                Volver a analizar
              </button>
            </div>
          </div>
        )}

        {(created || orders.length > 0) && (
          <div className={styles.cardPadCol} style={{ border: '1px solid var(--line)', borderRadius: 10, background: 'var(--panel)' }}>
            {created && (
              <div className={styles.row}>
                <span className={styles.chipGrn}>{local ? '✓ guardadas en local' : `✓ creadas en ${providerShort(project)}`}</span>
                <span className={styles.hint}>
                  {created[0]?.key}…{created[created.length - 1]?.key} en {providerWhere(project)}
                  {local ? '' : ', asignadas a ti'}. Ya forman la cola del proyecto, en este orden.
                </span>
              </div>
            )}
            {local && unexported > 0 && (
              <div className={styles.laterBox}>
                <span className={styles.hint}>
                  <b>¿Proveedor más adelante?</b> Cuando conectes ClickUp o GitHub, Hangar exporta las órdenes que nadie empezó y desde entonces viven allí. Las ya hechas
                  se quedan en el historial.
                </span>
                <span className={styles.spacer} />
                <button className={`${styles.btn} ${styles.sm}`} onClick={onToProvider}>
                  Conectar ahora
                </button>
              </div>
            )}
            {!local && unexported > 0 && (
              <div className={styles.laterBox}>
                <span className={styles.hint}>{unexported} órdenes viven solo en Hangar.</span>
                <span className={styles.spacer} />
                <button className={`${styles.btn} ${styles.sm}`} onClick={exportLater}>
                  Exportarlas a {providerShort(project)}
                </button>
              </div>
            )}
            <QueueList orders={orders} providerShort={providerShort(project)} onReorder={reorder} />
          </div>
        )}
      </div>
      <StepFoot
        step={4}
        hint={pending ? `${pending} órdenes en la cola del proyecto.` : 'Trae las tareas del proveedor o analiza un TRD y aprueba el desglose.'}
        onBack={onBack}
        onNext={onNext}
        nextDisabled={pending === 0}
      />
    </>
  );
}
