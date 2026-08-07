import { useCallback, useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { toastError } from '../components/ErrorOverlay';
import type { Requirement } from '../types/requirements';
import styles from './RequirementsPanel.module.css';

/**
 * Lo que Hangar necesita en la máquina, y lo que falta.
 *
 * Hangar orquesta CLIs de otros, así que en un equipo recién instalado casi
 * todo está inerte hasta que existan. Hasta ahora eso aparecía en el momento de
 * usarlo: una terminal que no abre, un botón de PR que da error, una pista de
 * instalación que nombra una herramienta que tampoco tienes.
 *
 * Dos reglas que esta pantalla existe para cumplir:
 *
 * - **Nombrar el paso que falta de verdad.** Si no tienes `uv`, la pista del
 *   grafo es `brew install uv`, no un comando que va a fallar.
 * - **Decir qué te da.** Una lista de binarios es una tarea; una lista de lo
 *   que desbloquea cada uno es una decisión.
 */
export function RequirementsPanel() {
  const [reqs, setReqs] = useState<Requirement[]>([]);
  const [checking, setChecking] = useState(false);

  const load = useCallback(
    () => invoke<Requirement[]>('list_requirements').then(setReqs).catch((e) => toastError(String(e))),
    [],
  );

  useEffect(() => {
    load();
  }, [load]);

  const recheck = async () => {
    setChecking(true);
    try {
      // Vuelve a mirar el disco olvidando la caché de rutas: si no, instalas lo
      // que te dijo y te sigue diciendo que falta hasta reiniciar la app.
      setReqs(await invoke<Requirement[]>('recheck_requirements'));
    } catch (e) {
      toastError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const missingRequired = reqs.filter((r) => !r.installed && r.need === 'required');
  const missingOptional = reqs.filter((r) => !r.installed && r.need === 'optional');

  return (
    <div className={styles.wrap}>
      <div className={styles.head}>
        <span className={styles.summary}>
          {missingRequired.length > 0 ? (
            <>
              Falta{missingRequired.length === 1 ? '' : 'n'} <strong>{missingRequired.length}</strong> herramienta
              {missingRequired.length === 1 ? '' : 's'} imprescindible{missingRequired.length === 1 ? '' : 's'}.
            </>
          ) : missingOptional.length > 0 ? (
            <>
              Todo lo imprescindible está. Hay <strong>{missingOptional.length}</strong> extra
              {missingOptional.length === 1 ? '' : 's'} sin instalar — opcionales de verdad.
            </>
          ) : (
            <>Todo instalado.</>
          )}
        </span>
        <button className={styles.btn} onClick={recheck} disabled={checking}>
          {checking ? 'comprobando…' : 'volver a comprobar'}
        </button>
      </div>

      <div className={styles.list}>
        {reqs.map((r) => (
          <div key={r.id} className={styles.row}>
            <span className={r.installed ? styles.dotOk : r.need === 'required' ? styles.dotBad : styles.dotOpt} />
            <div className={styles.info}>
              <div className={styles.title}>
                <span className={styles.label}>{r.label}</span>
                {!r.installed && r.need === 'required' && <span className={styles.tagRequired}>imprescindible</span>}
                {!r.installed && r.need === 'optional' && <span className={styles.tagOptional}>opcional</span>}
              </div>
              <div className={styles.unlocks}>{r.unlocks}</div>

              {r.installed ? (
                // La ruta, no solo un tick: «funciona en mi terminal» y «Hangar
                // lo encuentra» son cosas distintas cuando la app se abre desde
                // Finder con un PATH mínimo.
                <div className={styles.path}>{r.path}</div>
              ) : (
                <div className={styles.hintWrap}>
                  {r.blockedBy && <div className={styles.blocked}>Antes hace falta {r.blockedBy}:</div>}
                  <code className={styles.hint}>{r.installHint}</code>
                </div>
              )}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
