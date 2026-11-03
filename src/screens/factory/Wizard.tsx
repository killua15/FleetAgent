import { Fragment, type ReactNode } from 'react';
import { FACTORY_STEPS } from '../../types/factory';
import styles from '../Factory.module.css';

/** The five setup steps. A step you've reached can be revisited; the ones
 *  ahead open as the step before is done. */
export function Wizard({ step, reached, onGo, children }: { step: number; reached: number; onGo: (n: number) => void; children: ReactNode }) {
  return (
    <section className={styles.setup}>
      <div className={styles.stepper}>
        {FACTORY_STEPS.map((label, i) => {
          const n = i + 1;
          const cls = n === step ? styles.stepOn : n < step || n <= reached ? styles.stepDone : styles.stepLocked;
          return (
            <Fragment key={label}>
              {i > 0 && <span className={styles.stepLine} />}
              <button className={cls} disabled={n > reached || n === step} onClick={() => onGo(n)} aria-current={n === step ? 'step' : undefined}>
                <span className={styles.stepN}>{n < step ? '✓' : n}</span>
                {label}
              </button>
            </Fragment>
          );
        })}
      </div>
      {children}
    </section>
  );
}

/** The bar under every step: back, what's missing, next. */
export function StepFoot({
  step,
  hint,
  onBack,
  onNext,
  nextDisabled,
  nextLabel = 'Siguiente →',
  extra,
}: {
  step: number;
  hint: string;
  onBack?: () => void;
  onNext?: () => void;
  nextDisabled?: boolean;
  nextLabel?: string;
  extra?: ReactNode;
}) {
  return (
    <div className={styles.stepFoot}>
      <button className={styles.btn} disabled={!onBack || step === 1} onClick={onBack}>
        ← Atrás
      </button>
      <span className={styles.hint}>{hint}</span>
      <span className={styles.spacer} />
      {extra}
      {onNext && (
        <button className={styles.btnPrimary} disabled={nextDisabled} onClick={onNext}>
          {nextLabel}
        </button>
      )}
    </div>
  );
}
