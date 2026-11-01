import { useState } from 'react';
import { invoke } from '../../lib/invoke';
import { toastSuccess } from '../../components/ErrorOverlay';
import { ConfirmDeleteDialog } from '../../components/ConfirmDeleteDialog';
import { MODE_LABEL, providerShort, tildePath, type FactoryProject } from '../../types/factory';
import styles from '../Factory.module.css';

/** The project's name and where it lives, with what's been decided about it
 *  so far as chips. A project still being created has only a title. */
export function ProjectHeader({ project, home, onRemoved }: { project: FactoryProject | null; home: string | null; onRemoved: (projectId: string) => void }) {
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
      <span className={styles.spacer} />
      <RemoveProject project={project} home={home} onRemoved={onRemoved} />
    </div>
  );
}

/** «Eliminar»: out of the Factory, and — only on «Sí» — the folder to the
 *  Trash and out of Hangar. «No» is the default. */
function RemoveProject({ project, home, onRemoved }: { project: FactoryProject; home: string | null; onRemoved: (projectId: string) => void }) {
  const [folder, setFolder] = useState(false);
  const where = tildePath(project.projectPath, home);
  const remove = async () => {
    const done = await invoke<{ worktrees: number; folderTrashed: boolean }>('factory_remove_project', { projectId: project.projectId, deleteFolder: folder });
    toastSuccess(done.folderTrashed ? `«${project.projectName}» eliminado · ${where} está en la Papelera` : `«${project.projectName}» salió de la Factory · la carpeta sigue en ${where}`);
    setFolder(false);
    onRemoved(project.projectId);
  };
  return (
    <ConfirmDeleteDialog
      trigger={
        <button className={`${styles.btnGhost} ${styles.sm} ${styles.danger}`} title="Quitar este proyecto de la Factory">
          Eliminar
        </button>
      }
      title={`Eliminar «${project.projectName}» de la Factory`}
      description="Se para su línea y se borran sus órdenes, ejecuciones, equipo, brain y bandeja de enmiendas, y los worktrees de sus órdenes. No se puede deshacer."
      confirmLabel="Eliminar"
      cancelLabel="Cancelar"
      onConfirm={remove}
    >
      <fieldset className={styles.removeChoice}>
        <legend>
          ¿Borrar también la carpeta <span className={styles.mono}>{where}</span>?
        </legend>
        <label>
          <input type="radio" name="remove-folder" checked={!folder} onChange={() => setFolder(false)} />
          <span>
            <b>No</b> — la carpeta, su repo y sus ramas se quedan; el proyecto sigue en Hangar.
          </span>
        </label>
        <label>
          <input type="radio" name="remove-folder" checked={folder} onChange={() => setFolder(true)} />
          <span>
            <b>Sí</b> — la carpeta va a la Papelera (se puede recuperar de ahí) y el proyecto sale también de Hangar.
          </span>
        </label>
      </fieldset>
    </ConfirmDeleteDialog>
  );
}
