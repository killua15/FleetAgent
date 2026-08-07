import { useCallback, useEffect, useState } from "react";
import { invoke } from "../lib/invoke";
import { toastError } from "../components/ErrorOverlay";
import type { CapabilityStatus } from "../types/requirements";
import styles from "./RequirementsPanel.module.css";

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
    () =>
      invoke<CapabilityStatus[]>("list_requirements")
        .then(setCaps)
        .catch((e) => toastError(String(e))),
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
      setCaps(await invoke<CapabilityStatus[]>("recheck_requirements"));
    } catch (e) {
      toastError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const working = caps.filter((c) => c.available);
  // Lo imprescindible primero: si falta, es lo único que importa de esta
  // pantalla y no debería estar por debajo de un extra opcional.
  const broken = caps
    .filter((c) => !c.available)
    .sort((a, b) => Number(b.essential) - Number(a.essential));
  const brokenEssential = broken.filter((c) => c.essential);
  const brokenOptional = broken.filter((c) => !c.essential);

  return (
    <div className={styles.wrap}>
      <div className={styles.head}>
        <span className={styles.summary}>
          {brokenEssential.length > 0 ? (
            <>
              Falta lo imprescindible para{" "}
              <strong>
                {brokenEssential.map((c) => c.label.toLowerCase()).join(" y ")}
              </strong>
              .
            </>
          ) : brokenOptional.length > 0 ? (
            <>
              Lo esencial funciona. Hay <strong>{brokenOptional.length}</strong>{" "}
              cosa{brokenOptional.length === 1 ? "" : "s"} que podrías hacer y
              todavía no — opcionales de verdad.
            </>
          ) : (
            <>Todo disponible.</>
          )}
        </span>
        <button className={styles.btn} onClick={recheck} disabled={checking}>
          {checking ? "comprobando…" : "volver a comprobar"}
        </button>
      </div>

      {/* Lo que funciona se resume en una línea. Nueve tarjetas para decir
          nueve veces "esto va bien" es scroll a cambio de nada, y entierra lo
          único que pide atención. La ruta de cada binario sigue estando, en el
          tooltip: «funciona en mi terminal» y «Hangar lo encuentra» son cosas
          distintas cuando la app se abre desde Finder con un PATH mínimo. */}
      {working.length > 0 && (
        <div className={styles.okStrip}>
          {working.map((cap) => (
            <span
              key={cap.id}
              className={styles.okChip}
              title={`${cap.enables}\n\n${cap.tools.map((t) => `${t.name} → ${t.path}`).join("\n")}`}
            >
              ✓ {cap.label}
            </span>
          ))}
        </div>
      )}

      {/* Y lo que falta ocupa el ancho, en rejilla: es lo que hay que leer. */}
      <div className={styles.grid}>
        {broken.map((cap) => (
          <div
            key={cap.id}
            className={cap.essential ? styles.cardBad : styles.card}
          >
            <div className={styles.title}>
              <span className={cap.essential ? styles.dotBad : styles.dotOpt} />
              <span className={styles.label}>{cap.label}</span>
              <span
                className={
                  cap.essential ? styles.tagRequired : styles.tagOptional
                }
              >
                {cap.essential ? "imprescindible" : "opcional"}
              </span>
            </div>
            <div className={styles.unlocks}>{cap.enables}</div>

            <details className={styles.guide} open>
              <summary className={styles.guideHead}>
                {cap.reason} — cómo arreglarlo
              </summary>
              <div className={styles.guideBody}>
                {cap.tools
                  .filter((t) => !t.installed)
                  .map((tool) => (
                    <div key={tool.name} className={styles.step}>
                      <div className={styles.stepName}>
                        {tool.name}
                        {tool.blockedBy && (
                          <span className={styles.needs}>
                            {" "}
                            — antes hace falta {tool.blockedBy}
                          </span>
                        )}
                      </div>
                      {tool.installHint && (
                        <code className={styles.hint}>{tool.installHint}</code>
                      )}
                      {tool.alternative && (
                        <div className={styles.alt}>
                          o, si no usas Homebrew:{" "}
                          <code className={styles.hintInline}>
                            {tool.alternative}
                          </code>
                        </div>
                      )}
                      {tool.then && (
                        <div className={styles.after}>{tool.then}</div>
                      )}
                    </div>
                  ))}
              </div>
            </details>
          </div>
        ))}
      </div>

      {broken.length > 0 && (
        <p className={styles.footnote}>
          Cuando termines, pulsa <strong>volver a comprobar</strong> — Hangar
          cachea dónde encuentra cada binario, y si no seguiría diciéndote que
          falta.
        </p>
      )}

      <p className={styles.footnote}>
        El estado de sesión de GitHub no se comprueba aquí — eso lanza un
        proceso, y abrir Ajustes no debería esperar por la red. Te lo dice la
        función que lo necesite, cuando lo necesite.
      </p>
    </div>
  );
}
