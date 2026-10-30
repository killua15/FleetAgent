import { Fragment, type ReactNode } from 'react';
import { FACTORY_STEPS, STEP_PHASE } from '../../types/factory';
import styles from '../Factory.module.css';

/** The five setup steps. Only the first one exists yet; the rest show where
 *  they're headed and which phase brings them. */
export function Wizard({ children }: { children: ReactNode }) {
  return (
    <section className={styles.setup}>
      <div className={styles.stepper}>
        {FACTORY_STEPS.map((label, i) => {
          const n = i + 1;
          const locked = n > 1;
          return (
            <Fragment key={label}>
              {i > 0 && <span className={styles.stepLine} />}
              <span className={locked ? styles.stepLocked : styles.stepOn} title={locked ? `llega en ${STEP_PHASE[n]}` : undefined} aria-current={locked ? undefined : 'step'}>
                <span className={styles.stepN}>{n}</span>
                {label}
                {locked && <span className={styles.soon}>{STEP_PHASE[n]}</span>}
              </span>
            </Fragment>
          );
        })}
      </div>
      {children}
    </section>
  );
}
