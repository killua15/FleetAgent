import { MODE_LABEL, providerShort, tildePath, type FactoryProject } from '../../types/factory';
import styles from '../Factory.module.css';

/** The project's name and where it lives, with what's been decided about it
 *  so far as chips. A project still being created has only a title. */
export function ProjectHeader({ project, home }: { project: FactoryProject | null; home: string | null }) {
  if (!project) {
    return (
      <div className={styles.head}>
        <div className={styles.titleBlock}>
          <span className={styles.title}>Nuevo proyecto</span>
          <span className={styles.path}>elige su carpeta y cómo trabaja</span>
        </div>
      </div>
    );
  }
  return (
    <div className={styles.head}>
      <div className={styles.titleBlock}>
        <span className={styles.title}>{project.projectName}</span>
        <span className={styles.path}>{tildePath(project.projectPath, home)}</span>
      </div>
      {project.brainVersion ? (
        <span className={styles.chipAcc}>📖 Brain v{project.brainVersion} · sellado</span>
      ) : (
        <span className={styles.chip}>📖 Brain · sin definir</span>
      )}
      <span className={styles.chip}>modo {MODE_LABEL[project.mode].toLowerCase()}</span>
      <span className={styles.chip}>entrega · {project.delivery === 'mr' ? 'MR → dev' : 'merge local'}</span>
      <span className={styles.chip}>tareas · {providerShort(project)}</span>
    </div>
  );
}
