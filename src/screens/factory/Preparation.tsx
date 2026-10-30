import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import { useNavigation } from '../../nav/NavigationProvider';
import type { PrepCheck } from '../../types/factory';
import styles from '../Factory.module.css';

const ICON: Record<PrepCheck['status'], [string, string]> = {
  ok: ['✓', styles.ok],
  fixed: ['✓', styles.ok],
  todo: ['↻', styles.accTxt],
  warn: ['⚠', styles.warn],
  blocked: ['✗', styles.bad],
};

/**
 * «Preparación»: everything the line needs from the project, checked — and
 * what can be fixed without asking (git, dev, the brain's index, order 0),
 * fixed on «Preparar ahora» or on ▶. What needs you says where.
 */
export function Preparation({
  projectId,
  onGoStep,
  onChange,
  onPlay,
}: {
  projectId: string;
  onGoStep: (step: number) => void;
  onChange?: (checks: PrepCheck[]) => void;
  /** Shown as ▶ next to «Preparar ahora» — in «En vivo», where ▶ was refused. */
  onPlay?: () => void;
}) {
  const { navigate } = useNavigation();
  const [checks, setChecks] = useState<PrepCheck[] | null>(null);
  const [working, setWorking] = useState(false);
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;

  const run = useCallback(
    async (fix: boolean) => {
      setWorking(true);
      try {
        const found = await invoke<PrepCheck[]>('factory_prepare', { projectId, fix });
        setChecks(found);
        onChangeRef.current?.(found);
        if (fix && found.some((c) => c.status === 'fixed')) toastSuccess('Proyecto preparado');
      } catch (e) {
        toastError(String(e));
      } finally {
        setWorking(false);
      }
    },
    [projectId],
  );

  useEffect(() => {
    run(false);
  }, [run]);

  const act = async (action: NonNullable<PrepCheck['action']>) => {
    if (action === 'apply_testers') {
      try {
        await invoke('factory_team_apply_suggestions', { projectId });
        toastSuccess('Testers con los comandos del brain');
        run(false);
      } catch (e) {
        toastError(String(e));
      }
    } else if (action === 'team') onGoStep(2);
    else if (action === 'brain') onGoStep(3);
    else if (action === 'tasks') onGoStep(4);
    else if (action === 'git_identity') navigate('git-panel');
  };

  const ACTION_LABEL: Record<NonNullable<PrepCheck['action']>, string> = {
    apply_testers: 'Aplicar los del brain',
    team: 'Ir a Equipo',
    brain: 'Ir a Brain',
    tasks: 'Ir a Tareas',
    git_identity: 'Configurar',
  };

  const pending = checks?.some((c) => c.status === 'todo');

  return (
    <div className={styles.cardFlat}>
      <div className={styles.cardHead}>
        <span className={styles.sectionLabel}>PREPARACIÓN</span>
        <span className={styles.hint}>lo que la línea necesita del proyecto; lo que se puede arreglar solo, se arregla</span>
        <span className={styles.spacer} />
        <button className={`${pending ? styles.btnPrimary : styles.btn} ${styles.sm}`} disabled={working} onClick={() => run(true)}>
          {working ? 'preparando…' : 'Preparar ahora'}
        </button>
        {onPlay && (
          <button
            className={`${styles.btnPrimary} ${styles.sm}`}
            disabled={working || !checks || checks.some((c) => c.status === 'blocked')}
            title={checks?.some((c) => c.status === 'blocked') ? 'Resuelve antes lo marcado con ✗' : undefined}
            onClick={onPlay}
          >
            ▶ Play
          </button>
        )}
      </div>
      <div className={styles.col} style={{ gap: 6, padding: '0 16px 14px' }}>
        {!checks && <span className={styles.hint}>comprobando…</span>}
        {checks?.map((c, i) => {
          const [icon, cls] = ICON[c.status];
          return (
            <div key={`${c.id}-${i}`} className={styles.prepRow}>
              <span className={cls}>{icon}</span>
              <span className={styles.checkKey}>{c.label}</span>
              <span className={c.status === 'blocked' ? styles.bad : styles.hint}>{c.detail}</span>
              {c.action && c.status !== 'ok' ? (
                <button className={`${styles.btn} ${styles.sm}`} onClick={() => act(c.action!)}>
                  {ACTION_LABEL[c.action]}
                </button>
              ) : (
                <span />
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
