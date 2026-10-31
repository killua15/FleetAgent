import type { FactoryProject } from '../../types/factory';
import styles from '../Factory.module.css';

/** What a project is doing, in the sidebar's few words. */
function stateLine(p: FactoryProject): string {
  if (p.setupStep < 6) return `configurando · paso ${p.setupStep} de 5`;
  return { running: 'trabajando', paused: 'en pausa · termina la orden', stopped: 'parada' }[p.loopState];
}

function dotClass(p: FactoryProject): string {
  if (p.setupStep < 6) return styles.dotCfg;
  return p.loopState === 'running' ? styles.dotRun : p.loopState === 'paused' ? styles.dotWait : styles.dot;
}

/** Only the projects that joined the Factory — not every folder Hangar knows. */
export function ProjectSidebar({
  projects,
  selectedId,
  creating,
  onSelect,
  onNew,
  onStopAll,
}: {
  projects: FactoryProject[];
  selectedId: string | null;
  creating: boolean;
  onSelect: (projectId: string) => void;
  onNew: () => void;
  /** ⏹ for every project at once. */
  onStopAll: () => void;
}) {
  return (
    <aside className={styles.side}>
      <span className={styles.sideTitle}>PROYECTOS</span>
      {projects.map((p) => (
        <button
          key={p.projectId}
          className={!creating && p.projectId === selectedId ? styles.projectOn : styles.project}
          onClick={() => onSelect(p.projectId)}
          title={p.projectPath}
        >
          <span className={styles.projectName}>
            <span className={dotClass(p)} />
            <span>{p.projectName}</span>
          </span>
          <span className={styles.projectState}>{stateLine(p)}</span>
        </button>
      ))}
      <button className={creating ? `${styles.newProject} ${styles.newProjectOn}` : styles.newProject} onClick={onNew}>
        + Nuevo proyecto
      </button>
      <span className={styles.spacer} />
      {projects.some((p) => p.setupStep >= 6 && p.loopState !== 'stopped') && (
        <button className={styles.stopAll} title="Para la línea de todos los proyectos y mata sus procesos" onClick={onStopAll}>
          ⏹ Parar todo
        </button>
      )}
    </aside>
  );
}
