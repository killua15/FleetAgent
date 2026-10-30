import { useCallback, useEffect, useState } from 'react';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import { BRAIN_SECTIONS, levelLabel, type Amendment, type BrainReport, type FactoryProject, type SealedBrain } from '../../types/factory';
import { ImportReport } from './BrainImport';
import { BrainSession } from './BrainSession';
import styles from '../Factory.module.css';

const FORMATS: { format: 'brain' | 'agents' | 'claude'; file: string }[] = [
  { format: 'brain', file: 'BRAIN.md' },
  { format: 'agents', file: 'AGENTS.md' },
  { format: 'claude', file: 'CLAUDE.md' },
];

/**
 * The Brain tab: the sealed rules by section, from the start of a project to
 * production; export to the three formats; the amendments inbox; and the
 * amendment session that turns vN plus the inbox into vN+1.
 */
export function BrainView({ project, onSealed }: { project: FactoryProject; onSealed: () => void }) {
  const [brain, setBrain] = useState<SealedBrain | null>(null);
  const [inbox, setInbox] = useState<Amendment[]>([]);
  const [section, setSection] = useState('arq');
  const [amending, setAmending] = useState(false);
  const [report, setReport] = useState<BrainReport | null>(null);
  const [sealing, setSealing] = useState(false);

  const load = useCallback(() => {
    invoke<SealedBrain | null>('factory_brain_get', { projectId: project.projectId, version: null })
      .then(setBrain)
      .catch((e) => toastError(String(e)));
    invoke<Amendment[]>('factory_amendments_list', { projectId: project.projectId })
      .then(setInbox)
      .catch(() => setInbox([]));
  }, [project.projectId]);

  useEffect(load, [load, project.brainVersion]);

  const exportAs = async (format: string, file: string) => {
    const path = await saveDialog({ defaultPath: `${project.projectPath}/${file}`, title: `Exportar ${file}` });
    if (!path) return;
    try {
      await invoke('factory_brain_export', { projectId: project.projectId, format, path });
      toastSuccess(`${file} exportado`);
    } catch (e) {
      toastError(String(e));
    }
  };

  const seal = async () => {
    if (!report) return;
    setSealing(true);
    try {
      const next = await invoke<SealedBrain>('factory_brain_seal', { projectId: project.projectId, markdown: report.markdown, source: 'amendment' });
      toastSuccess(`Brain v${next.version} sellado · las órdenes nuevas lo congelan; las que están en curso siguen con la suya`);
      setReport(null);
      setAmending(false);
      onSealed();
      load();
    } catch (e) {
      toastError(String(e));
    } finally {
      setSealing(false);
    }
  };

  const dismiss = async (id: string) => {
    try {
      await invoke('factory_amendment_set_status', { id, status: 'dismissed' });
      load();
    } catch (e) {
      toastError(String(e));
    }
  };

  if (!brain) return <span className={styles.hint}>Todavía no hay un brain sellado.</span>;

  const open = inbox.filter((a) => a.status === 'open');
  const rules = brain.rules.filter((r) => r.section === section);

  return (
    <div className={styles.col} style={{ gap: 16 }}>
      <div className={styles.row}>
        <span className={styles.chipAcc}>BRAIN.md · v{brain.version} · sellado</span>
        <span className={styles.num}>
          {brain.rules.length} reglas · {new Set(brain.rules.map((r) => r.section)).size} secciones · {brain.rules.filter((r) => r.check).length} con check:
        </span>
        <span className={styles.spacer} />
        {FORMATS.map(({ format, file }) => (
          <button key={format} className={`${styles.btn} ${styles.sm}`} onClick={() => exportAs(format, file)}>
            {format === 'brain' ? `Exportar ${file}` : file}
          </button>
        ))}
        <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={() => setAmending(!amending)}>
          {amending ? 'Cerrar la enmienda' : `Sesión de enmienda → v${brain.version + 1}`}
        </button>
      </div>

      {amending && (
        <div className={styles.session}>
          <BrainSession
            projectId={project.projectId}
            amend
            onGenerated={async (markdown) => {
              try {
                setReport(await invoke<BrainReport>('factory_brain_parse', { markdown }));
              } catch (e) {
                toastError(String(e));
              }
            }}
          />
          <ImportReport report={report} from="enmienda" nextVersion={brain.version + 1} sealing={sealing} onSeal={seal} />
        </div>
      )}

      <div className={styles.brainGrid}>
        <div className={`${styles.cardFlat} ${styles.secList}`}>
          <span className={styles.secGroup}>Del inicio…</span>
          {BRAIN_SECTIONS.map(([key, name], i) => (
            <span key={key} className={styles.col} style={{ gap: 0 }}>
              {i === 6 && <span className={styles.secGroup}>…a producción</span>}
              <button className={key === section ? styles.secOn : styles.sec} onClick={() => setSection(key)}>
                {name} <span className={styles.num}>{brain.rules.filter((r) => r.section === key).length}</span>
              </button>
            </span>
          ))}
        </div>
        <div className={styles.col} style={{ gap: 16 }}>
          <div className={styles.cardFlat}>
            {rules.length === 0 && <span className={`${styles.hint} ${styles.cardPadCol}`}>Esta sección no tiene reglas.</span>}
            {rules.map((r) => (
              <div key={r.id} className={styles.ruleRow}>
                <span className={styles.ruleId}>{r.id}</span>
                <span className={r.level === 'must' ? styles.lvlMust : styles.lvlShould}>{levelLabel(r.level)}</span>
                <div>
                  <div className={styles.ruleTxt}>{r.text}</div>
                  {r.check && <div className={styles.check}>check: {r.check}</div>}
                </div>
              </div>
            ))}
          </div>
          <div className={styles.cardFlat}>
            <div className={styles.cardHead}>
              <span className={styles.sectionLabel}>BANDEJA DE ENMIENDAS</span>
              <span className={open.length ? styles.chipAmber : styles.chip}>{open.length} pendiente{open.length === 1 ? '' : 's'}</span>
            </div>
            {inbox.length === 0 && <span className={`${styles.hint} ${styles.cardPadCol}`}>Lo que el brain no cubra en una orden llegará aquí.</span>}
            {inbox.map((a) => (
              <div key={a.id} className={styles.inbox} style={a.status === 'open' ? undefined : { opacity: 0.55 }}>
                <span className={styles.ruleTag}>{a.orderKey ?? '—'}</span>
                <span className={styles.hint}>{a.question}</span>
                <span className={styles.num}>
                  {a.status === 'open' ? (a.provisional ? `decidido: ${a.provisional}` : 'abierta') : a.status === 'included' ? `en v${a.includedIn}` : 'descartada'}
                </span>
                {a.status === 'open' && (
                  <button className={`${styles.btnGhost} ${styles.sm}`} onClick={() => dismiss(a.id)}>
                    descartar
                  </button>
                )}
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
