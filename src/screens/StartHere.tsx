import { useCallback, useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { Spinner } from '../components/Spinner';
import { toastError } from '../components/ErrorOverlay';
import { useNavigation, type ScreenName } from '../nav/NavigationProvider';
import { STEP_COPY, type OnboardingProgress, type OnboardingStep } from '../types/onboarding';
import styles from './StartHere.module.css';

/**
 * Lo que Hangar hace, en el orden en que empieza a importar.
 *
 * No es un tour: es el estado real leído de la base de datos, así que sigue
 * sirviendo el día veinte como panel de "qué me falta por probar". Un tour se ve
 * una vez y a la semana miente; esto no puede mentir porque no se lo inventa.
 *
 * Los nueve primeros pasos cuentan la historia del producto en orden, y los tres
 * del medio —  segunda cuenta, rotación activada, ha rotado— son el eje donde
 * Hangar está solo. El resto va en un grupo aparte para que la primera lista se
 * lea de un tirón.
 */

/** Dónde se hace cada cosa. Fuera de `STEP_COPY` porque son rutas de la app y
 * no texto: si una pantalla se renombra, esto tiene que romper la compilación. */
const DESTINATION: Record<string, ScreenName> = {
  requirements: 'general-settings',
  first_account: 'account-list',
  first_project: 'project-settings',
  first_worktree: 'terminals',
  second_account: 'account-list',
  rotation_on: 'rotation-settings',
  spend_visible: 'terminals',
  budget_set: 'general-settings',
  queue_used: 'queue',
  skills_on: 'skill-settings',
  pr_inbox: 'pr-reviews',
  graph_built: 'graph-groups',
};

export function StartHere() {
  const { navigate } = useNavigation();
  const [progress, setProgress] = useState<OnboardingProgress | null>(null);

  const reload = useCallback(
    () =>
      invoke<OnboardingProgress>('onboarding_progress')
        .then(setProgress)
        .catch((e) => toastError(String(e))),
    [],
  );

  useEffect(() => {
    void reload();
  }, [reload]);

  if (!progress) {
    return (
      <div className={styles.screen}>
        <AppTopBar />
        <div className={styles.loading}>
          <Spinner label="mirando qué llevas hecho…" />
        </div>
      </div>
    );
  }

  const allBasicsDone = progress.basicsDone === progress.basicsTotal;

  return (
    <div className={styles.screen}>
      <AppTopBar />
      <div className={styles.body}>
        <header className={styles.header}>
          <h1 className={styles.title}>Empieza aquí</h1>
          <p className={styles.subtitle}>
            {allBasicsDone
              ? 'Lo esencial está hecho. Lo de abajo es lo que queda por descubrir.'
              : 'Cada paso dice para qué sirve. No hay que hacerlos en orden, pero el orden es el que tiene sentido.'}
          </p>
          <div className={styles.meter}>
            <div className={styles.meterFill} style={{ width: `${(progress.basicsDone / progress.basicsTotal) * 100}%` }} />
          </div>
          <span className={styles.count}>
            {progress.basicsDone} de {progress.basicsTotal}
          </span>
        </header>

        <section className={styles.group}>
          <span className={styles.groupLabel}>LO ESENCIAL</span>
          {progress.basics.map((s) => (
            <StepRow key={s.id} step={s} onGo={navigate} />
          ))}
        </section>

        <section className={styles.group}>
          <span className={styles.groupLabel}>Y ADEMÁS</span>
          <p className={styles.groupNote}>Nada de esto hace falta para trabajar. Está aquí porque si no, no se encuentra.</p>
          {progress.beyond.map((s) => (
            <StepRow key={s.id} step={s} onGo={navigate} />
          ))}
        </section>
      </div>
    </div>
  );
}

function StepRow({ step, onGo }: { step: OnboardingStep; onGo: (screen: ScreenName) => void }) {
  // Un paso sin copia es un id que Rust envía y el frontend no conoce: se
  // enseña crudo en vez de desaparecer, porque desaparecer lo hace invisible
  // justo para quien tendría que arreglarlo.
  const copy = STEP_COPY[step.id];
  const destination = DESTINATION[step.id];

  return (
    <div className={step.done ? styles.rowDone : styles.row}>
      <span className={step.done ? styles.markDone : styles.mark}>{step.done ? '✓' : '○'}</span>
      <div className={styles.rowBody}>
        <div className={styles.rowTop}>
          <span className={styles.rowTitle}>{copy?.title ?? step.id}</span>
          {step.detail && <span className={styles.detail}>{step.detail}</span>}
        </div>
        <span className={styles.why}>{copy?.why ?? 'Sin descripción — falta su entrada en STEP_COPY.'}</span>
      </div>
      {/* Los pasos hechos conservan su enlace: sirve para volver a mirar, no
          solo para completarlos. El que no tiene destino es `has_rotated`, que
          no se hace desde ninguna pantalla. */}
      {destination && (
        <button className={styles.go} onClick={() => onGo(destination)}>
          {copy?.where ?? 'ir'} →
        </button>
      )}
    </div>
  );
}
