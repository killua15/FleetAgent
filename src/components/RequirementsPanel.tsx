import { useCallback, useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { toastError } from '../components/ErrorOverlay';
import type { CapabilityStatus } from '../types/requirements';
import styles from './RequirementsPanel.module.css';

/**
 * Qué puede hacer Hangar en esta máquina, y qué falta para lo que no.
 *
 * La unidad es la **capacidad** —algo que quieres hacer—, no la herramienta.
 * Una lista de binarios es una tarea; una lista de lo que desbloquea cada uno
 * es una decisión. Y la frase que sale aquí es la misma que verás si intentas
 * usar la función directamente: las dos preguntan al mismo registro.
 *
 * Las herramientas salen debajo de cada capacidad, no sueltas, porque así se ve
 * *por qué* hace falta cada una en vez de recibir un comando a secas.
 */
export function RequirementsPanel() {
  const [caps, setCaps] = useState<CapabilityStatus[]>([]);
  const [checking, setChecking] = useState(false);

  const load = useCallback(
    () => invoke<CapabilityStatus[]>('list_requirements').then(setCaps).catch((e) => toastError(String(e))),
    [],
  );

  useEffect(() => {
    load();
  }, [load]);

  const recheck = async () => {
    setChecking(true);
    try {
      // Olvida la caché de rutas antes de mirar: si no, instalas lo que te dijo
      // y te sigue diciendo que falta hasta reiniciar la app.
      setCaps(await invoke<CapabilityStatus[]>('recheck_requirements'));
    } catch (e) {
      toastError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const brokenEssential = caps.filter((c) => !c.available && c.essential);
  const brokenOptional = caps.filter((c) => !c.available && !c.essential);

  return (
    <div className={styles.wrap}>
      <div className={styles.head}>
        <span className={styles.summary}>
          {brokenEssential.length > 0 ? (
            <>
              Falta lo imprescindible para <strong>{brokenEssential.map((c) => c.label.toLowerCase()).join(' y ')}</strong>.
            </>
          ) : brokenOptional.length > 0 ? (
            <>
              Lo esencial funciona. Hay <strong>{brokenOptional.length}</strong> cosa{brokenOptional.length === 1 ? '' : 's'} que
              podrías hacer y todavía no — opcionales de verdad.
            </>
          ) : (
            <>Todo disponible.</>
          )}
        </span>
        <button className={styles.btn} onClick={recheck} disabled={checking}>
          {checking ? 'comprobando…' : 'volver a comprobar'}
        </button>
      </div>

      <div className={styles.list}>
        {caps.map((cap) => (
          <div key={cap.id} className={styles.row}>
            <span className={cap.available ? styles.dotOk : cap.essential ? styles.dotBad : styles.dotOpt} />
            <div className={styles.info}>
              <div className={styles.title}>
                <span className={styles.label}>{cap.label}</span>
                {!cap.available && cap.essential && <span className={styles.tagRequired}>imprescindible</span>}
                {!cap.available && !cap.essential && <span className={styles.tagOptional}>opcional</span>}
              </div>
              <div className={styles.unlocks}>{cap.enables}</div>

              {!cap.available && (
                <div className={styles.hintWrap}>
                  <div className={styles.blocked}>{cap.reason}</div>
                  {cap.installHint && <code className={styles.hint}>{cap.installHint}</code>}
                </div>
              )}

              {/* Las herramientas, con su ruta cuando están: «funciona en mi
                  terminal» y «Hangar lo encuentra» son cosas distintas cuando
                  la app se abre desde Finder con un PATH mínimo. */}
              <div className={styles.tools}>
                {cap.tools.map((tool) => (
                  <span key={tool.name} className={tool.installed ? styles.toolOk : styles.toolMissing} title={tool.path ?? undefined}>
                    {tool.installed ? '✓' : '·'} {tool.name}
                    {tool.blockedBy && <span className={styles.needs}> ← necesita {tool.blockedBy}</span>}
                  </span>
                ))}
              </div>
            </div>
          </div>
        ))}
      </div>

      <p className={styles.footnote}>
        El estado de sesión de GitHub no se comprueba aquí — eso lanza un proceso, y abrir Ajustes no debería esperar por la red. Te
        lo dice la función que lo necesite, cuando lo necesite.
      </p>
    </div>
  );
}
