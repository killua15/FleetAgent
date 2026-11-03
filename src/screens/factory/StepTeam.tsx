import { useState } from 'react';
import type { FactoryProject, TeamView } from '../../types/factory';
import { TeamEditor } from './TeamEditor';
import { StepFoot } from './Wizard';
import styles from '../Factory.module.css';

/** Step 2 · Equipo: each agent with its account and model. */
export function StepTeam({ project, onBack, onNext }: { project: FactoryProject; onBack: () => void; onNext: () => void }) {
  const [view, setView] = useState<TeamView | null>(null);
  const on = view?.agents.filter((a) => a.active) ?? [];
  const testers = on.filter((a) => a.role === 'tester');
  const blocked = !view || view.check.problems.length > 0;
  return (
    <>
      <div className={styles.stepBody}>
        <div className={styles.row}>
          <span className={styles.lead}>El equipo: cada agente con su cuenta y su modelo</span>
          <span className={styles.spacer} />
          {view && (
            <span className={styles.hint}>
              {on.length} agentes activos · {testers.length} testers
            </span>
          )}
        </div>
        <span className={styles.hint}>
          Cada agente lee solo su parte del brain y cita la regla con la que decide. Mezcla modelos fuertes y baratos, y pon el aprobador en otra familia de modelo para que
          no se dé la razón a sí mismo. Si trabajas con una sola familia, puedes apagarlo: decide la puerta de tests.
        </span>
        <TeamEditor projectId={project.projectId} onChange={setView} />
      </div>
      <StepFoot
        step={2}
        hint={
          !view
            ? ''
            : testers.length === 0
              ? 'Activa al menos un tester: sin tests no se aprueba nada.'
              : blocked
                ? view.check.problems[0]
                : `${on.length} agentes · ${testers.length} testers · la puerta de tests es obligatoria.`
        }
        onBack={onBack}
        onNext={onNext}
        nextDisabled={blocked}
      />
    </>
  );
}
