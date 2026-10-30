import { useEffect, useState } from 'react';
import { invoke } from '../../lib/invoke';
import { toastError } from '../../components/ErrorOverlay';
import { MODE_LABEL, providerWhere, tildePath, type FactoryOrder, type FactoryProject, type SealedBrain, type TeamView } from '../../types/factory';
import { StepFoot } from './Wizard';
import styles from '../Factory.module.css';

/** Step 5 · ▶ Play: everything the line will work with, and the button. */
export function StepPlay({ project, home, onBack, onStarted }: { project: FactoryProject; home: string | null; onBack: () => void; onStarted: () => void }) {
  const [team, setTeam] = useState<TeamView | null>(null);
  const [brain, setBrain] = useState<SealedBrain | null>(null);
  const [orders, setOrders] = useState<FactoryOrder[]>([]);
  const [starting, setStarting] = useState(false);

  useEffect(() => {
    invoke<TeamView>('factory_team_get', { projectId: project.projectId }).then(setTeam).catch(() => setTeam(null));
    invoke<SealedBrain | null>('factory_brain_get', { projectId: project.projectId, version: null }).then(setBrain).catch(() => setBrain(null));
    invoke<FactoryOrder[]>('factory_orders_list', { projectId: project.projectId }).then(setOrders).catch(() => setOrders([]));
  }, [project.projectId]);

  const play = async () => {
    setStarting(true);
    try {
      await invoke('factory_play', { projectId: project.projectId });
      onStarted();
    } catch (e) {
      toastError(String(e));
    } finally {
      setStarting(false);
    }
  };

  const on = team?.agents.filter((a) => a.active) ?? [];
  const testers = on.filter((a) => a.role === 'tester');
  const approver = on.find((a) => a.role === 'approver');
  const todo = orders.filter((o) => o.status === 'todo');
  const first = todo.find((o) => o.blockedBy.length === 0);
  const ok = <span className={styles.ok}>✓</span>;
  const no = <span className={styles.bad}>✗</span>;

  return (
    <>
      <div className={styles.stepBody}>
        <span className={styles.lead}>Todo listo: así arranca la Factory</span>
        <div className={styles.checklist}>
          <div className={styles.checkItem}>
            {ok}
            <span className={styles.checkKey}>Proyecto</span>
            <span>{tildePath(project.projectPath, home)}</span>
          </div>
          <div className={styles.checkItem}>
            {ok}
            <span className={styles.checkKey}>Entrega y modo</span>
            <span>
              {project.delivery === 'mr' ? 'MR → dev' : 'merge local en dev'} · {MODE_LABEL[project.mode].toLowerCase()}
            </span>
          </div>
          <div className={styles.checkItem}>
            {team && !team.check.problems.length ? ok : no}
            <span className={styles.checkKey}>Equipo</span>
            <span>
              {on.length} agentes · testers: {testers.map((a) => `${a.short} (${a.model})`).join(', ') || 'ninguno'} · aprobador {approver?.model ?? '—'} · sin tests en verde no se aprueba
            </span>
          </div>
          <div className={styles.checkItem}>
            {brain ? ok : no}
            <span className={styles.checkKey}>Brain</span>
            <span>
              {brain ? (
                <>
                  <span className={styles.chipAcc}>v{brain.version} · sellado</span> {brain.rules.length} reglas · {new Set(brain.rules.map((r) => r.section)).size} secciones ·{' '}
                  {brain.rules.filter((r) => r.check).length} con check: · se exporta a BRAIN.md, AGENTS.md o CLAUDE.md
                </>
              ) : (
                'sin sellar'
              )}
            </span>
          </div>
          <div className={styles.checkItem}>
            {todo.length ? ok : no}
            <span className={styles.checkKey}>Tareas</span>
            <span>
              {todo.length} órdenes en {providerWhere(project)} · solo de este proyecto{first ? ` · empieza por ${first.key}` : ''}
            </span>
          </div>
          <div className={styles.checkItem}>
            <span className={styles.accTxt}>i</span>
            <span className={styles.checkKey}>Límites</span>
            <span>Si una cuenta agota su límite, la orden espera y sigue sola al renovarse; sin créditos te avisa.</span>
          </div>
        </div>
        <div className={styles.row} style={{ marginTop: 6 }}>
          <button className={`${styles.btnPrimary} ${styles.lg}`} disabled={starting} onClick={play}>
            {starting ? 'arrancando…' : '▶ Play'}
          </button>
          <span className={styles.hint}>{first ? `Toma ${first.key} y sigue orden tras orden, en el orden de la cola.` : 'Sin tareas, ▶ te dirá qué falta.'}</span>
        </div>
      </div>
      <StepFoot step={5} hint="Pulsa ▶ Play para arrancar." onBack={onBack} />
    </>
  );
}
