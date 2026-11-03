import { useEffect, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import type { DesignDirection, DesignReference } from '../../types/factory';
import styles from '../Factory.module.css';

/** The «Diseño» section's extras: the creator's visual references, and three
 *  directions the AI proposes to pick one from. */
export function DesignExtras({ projectId, onPick }: { projectId: string; onPick: (d: DesignDirection) => void }) {
  const [refs, setRefs] = useState<DesignReference[]>([]);
  const [directions, setDirections] = useState<DesignDirection[] | null>(null);
  const [asking, setAsking] = useState(false);

  useEffect(() => {
    invoke<DesignReference[]>('factory_design_refs', { projectId })
      .then(setRefs)
      .catch(() => setRefs([]));
  }, [projectId]);

  const add = async () => {
    const picked = await openDialog({ multiple: true, title: 'Imágenes de referencia', filters: [{ name: 'Imágenes', extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg'] }] });
    const paths = Array.isArray(picked) ? picked : typeof picked === 'string' ? [picked] : [];
    if (!paths.length) return;
    try {
      setRefs(await invoke<DesignReference[]>('factory_design_add_refs', { projectId, paths }));
    } catch (e) {
      toastError(String(e));
    }
  };
  const remove = async (name: string) => {
    try {
      setRefs(await invoke<DesignReference[]>('factory_design_remove_ref', { projectId, name }));
    } catch (e) {
      toastError(String(e));
    }
  };
  const propose = async () => {
    setAsking(true);
    try {
      setDirections(await invoke<DesignDirection[]>('factory_design_propose', { projectId }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setAsking(false);
    }
  };

  return (
    <div className={styles.decideFree}>
      <div className={styles.row}>
        <span className={styles.lName}>Referencias visuales</span>
        <span className={styles.hint}>capturas de apps o webs que te gusten: el diseñador, el builder y el revisor UI las miran</span>
        <span className={styles.spacer} />
        <button className={`${styles.btn} ${styles.sm}`} onClick={add}>
          + Añadir imágenes
        </button>
      </div>
      {refs.length > 0 ? (
        <div className={styles.refGrid}>
          {refs.map((r) => (
            <figure key={r.name} className={styles.refItem}>
              {r.dataUrl ? <img src={r.dataUrl} alt={r.name} /> : <span className={styles.hint}>{r.name}</span>}
              <figcaption>
                <span className={styles.lSub}>{r.name}</span>
                <button className={`${styles.btnGhost} ${styles.sm}`} title="Quitar" onClick={() => remove(r.name)}>
                  🗑
                </button>
              </figcaption>
            </figure>
          ))}
        </div>
      ) : (
        <span className={styles.hint}>Ninguna todavía.</span>
      )}

      <div className={styles.row}>
        <span className={styles.lName}>Direcciones visuales</span>
        <span className={styles.hint}>la IA lee el producto y tus referencias y propone tres estilos, cada uno con una pantalla de ejemplo</span>
        <span className={styles.spacer} />
        {asking ? (
          <>
            <span className={styles.hint}>diseñando… (unos minutos)</span>
            <button className={`${styles.btnGhost} ${styles.sm}`} onClick={() => invoke('factory_design_cancel', { projectId }).catch(() => undefined)}>
              Cancelar
            </button>
          </>
        ) : (
          <button className={`${styles.btn} ${styles.sm}`} onClick={propose}>
            ✦ Proponer estilos
          </button>
        )}
      </div>
      {directions && (
        <div className={styles.dirGrid}>
          {directions.map((d) => (
            <div key={d.name} className={styles.dirCard}>
              {/* No scripts: the preview is the AI's HTML, shown, never run. */}
              <iframe className={styles.dirPreview} sandbox="" srcDoc={d.previewHtml} title={d.name} />
              <div className={styles.dirInfo}>
                <span className={styles.lName}>{d.name}</span>
                <span className={styles.hint}>{d.mood}</span>
                <div className={styles.row} style={{ gap: 4, flexWrap: 'wrap' }}>
                  {d.colors.map((c) => {
                    const hex = c.match(/#[0-9a-fA-F]{3,8}/)?.[0];
                    return (
                      <span key={c} className={styles.swatch} title={c}>
                        <span style={{ background: hex ?? 'transparent' }} />
                        {c.replace(hex ?? '', '').trim()}
                      </span>
                    );
                  })}
                </div>
                <span className={styles.lSub}>
                  {d.font} · {d.shape} · {d.library}
                  {d.dark ? ' · con modo oscuro' : ''}
                </span>
                <button
                  className={`${styles.btnPrimary} ${styles.sm}`}
                  onClick={() => {
                    onPick(d);
                    toastSuccess(`«${d.name}» rellena la sección Diseño: revísala y ajústala`);
                  }}
                >
                  Elegir esta
                </button>
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
