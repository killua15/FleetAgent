import { useEffect, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import type { BrainReport, FactoryProject, SealedBrain } from '../../types/factory';
import { ImportReport } from './BrainImport';
import { BrainSession } from './BrainSession';
import { StepFoot } from './Wizard';
import styles from '../Factory.module.css';

type Mode = 'chat' | 'guide' | 'file';

/**
 * Step 3 · Brain: three ways to a BRAIN.md — a conversation here, a guide for
 * a conversation somewhere else, or a file you already have — and one way in:
 * the import report, then ✋ seal.
 */
export function StepBrain({
  project,
  onSealed,
  onBack,
  onNext,
}: {
  project: FactoryProject;
  onSealed: (sealed: SealedBrain) => void;
  onBack: () => void;
  onNext: () => void;
}) {
  const [mode, setMode] = useState<Mode>('chat');
  const [report, setReport] = useState<BrainReport | null>(null);
  const [from, setFrom] = useState('');
  const [draft, setDraft] = useState<string | null>(null);
  const [sealing, setSealing] = useState(false);
  const [guide, setGuide] = useState('');
  const [sealed, setSealed] = useState<SealedBrain | null>(null);

  useEffect(() => {
    invoke<string>('factory_brain_guide').then(setGuide).catch(() => setGuide(''));
    invoke<SealedBrain | null>('factory_brain_get', { projectId: project.projectId, version: null })
      .then(setSealed)
      .catch(() => setSealed(null));
  }, [project.projectId]);

  const show = async (markdown: string, label: string) => {
    try {
      setReport(await invoke<BrainReport>('factory_brain_parse', { markdown }));
      setFrom(label);
    } catch (e) {
      toastError(String(e));
    }
  };

  const pickFile = async () => {
    const path = await openDialog({ multiple: false, title: 'Elegir BRAIN.md', filters: [{ name: 'Markdown', extensions: ['md', 'markdown', 'txt'] }] });
    if (typeof path !== 'string') return;
    try {
      await show(await invoke<string>('factory_read_markdown', { path }), path.split('/').pop() ?? path);
    } catch (e) {
      toastError(String(e));
    }
  };

  const useExample = async () => show(await invoke<string>('factory_brain_example'), 'BRAIN.md de ejemplo');

  const seal = async () => {
    if (!report) return;
    setSealing(true);
    try {
      const next = await invoke<SealedBrain>('factory_brain_seal', {
        projectId: project.projectId,
        markdown: report.markdown,
        source: mode === 'chat' ? 'session' : mode === 'guide' ? 'guide' : 'import',
      });
      setSealed(next);
      setReport(null);
      onSealed(next);
      toastSuccess(`Brain v${next.version} sellado · ${next.rules.length} reglas`);
    } catch (e) {
      toastError(String(e));
    } finally {
      setSealing(false);
    }
  };

  const copyGuide = async () => {
    try {
      await navigator.clipboard.writeText(guide);
      toastSuccess('Guía copiada: pégala en tu conversación con la IA');
    } catch {
      toastError('No se pudo copiar: selecciona el texto y cópialo a mano');
    }
  };

  const completeInChat = () => {
    setDraft(report?.markdown ?? null);
    setMode('chat');
  };

  const nextVersion = (sealed?.version ?? 0) + 1;
  const hint = sealed ? `Brain v${sealed.version} sellado · ${sealed.rules.length} reglas.` : mode === 'chat' ? 'Conversa con la IA y genera el BRAIN.md.' : 'Importa el BRAIN.md, revisa el informe y séllalo.';

  return (
    <>
      <div className={styles.stepBody}>
        <div className={styles.row}>
          <span className={styles.lead}>El brain: lo defines en una conversación con la IA y lo sellas</span>
          <span className={styles.spacer} />
          {sealed && <span className={styles.chipAcc}>📖 v{sealed.version} · sellado</span>}
          <span className={styles.hint}>Empezar desde</span>
          <div className={styles.seg}>
            <button aria-pressed={mode === 'chat'} onClick={() => setMode('chat')}>
              Conversación aquí
            </button>
            <button aria-pressed={mode === 'guide'} onClick={() => setMode('guide')}>
              Desde otra conversación
            </button>
            <button aria-pressed={mode === 'file'} onClick={() => setMode('file')}>
              Importar .md
            </button>
          </div>
        </div>

        <div className={styles.session}>
          {mode === 'chat' ? (
            <BrainSession projectId={project.projectId} base={draft} onGenerated={(md) => show(md, 'generado en la sesión')} />
          ) : (
            <div className={styles.cardFlat}>
              {mode === 'guide' ? (
                <div className={styles.cardPadCol}>
                  <span className={styles.sectionLabel}>DESDE UNA CONVERSACIÓN QUE YA TIENES</span>
                  <span className={styles.hint}>
                    <span className={styles.numStep}>1</span>Copia esta guía y pégala al final de tu conversación con la IA (Claude, ChatGPT, Gemini…). Usa todo lo que ya
                    hablasteis.
                  </span>
                  <pre className={styles.guide}>{guide}</pre>
                  <div className={styles.row}>
                    <button className={`${styles.btn} ${styles.sm}`} onClick={copyGuide}>
                      ⧉ Copiar guía
                    </button>
                    <span className={styles.hint} style={{ fontSize: 11 }}>
                      Pide solo lo que se habló: lo que falte vuelve como preguntas.
                    </span>
                  </div>
                  <span className={styles.hint}>
                    <span className={styles.numStep}>2</span>La IA te devuelve un único <span className={styles.mono}>BRAIN.md</span>.{' '}
                    <span className={styles.numStep}>3</span>Impórtalo aquí:
                  </span>
                </div>
              ) : (
                <div className={styles.cardPadCol}>
                  <span className={styles.sectionLabel}>IMPORTAR UN .MD QUE YA TIENES</span>
                  <span className={styles.hint}>
                    Un <span className={styles.mono}>BRAIN.md</span> exportado de otro proyecto, o un <span className={styles.mono}>CLAUDE.md</span> /{' '}
                    <span className={styles.mono}>AGENTS.md</span>: Hangar reconoce las secciones y las reglas, y te dice qué falta.
                  </span>
                </div>
              )}
              <div className={styles.cardPad0}>
                <div className={styles.drop}>
                  <span style={{ fontSize: 22 }}>⇪</span>
                  <span className={styles.hint}>
                    Elige el <span className={styles.mono}>.md</span>
                  </span>
                  <div className={styles.row}>
                    <button className={`${styles.btn} ${styles.sm}`} onClick={pickFile}>
                      Elegir archivo…
                    </button>
                    <button className={`${styles.btnGhost} ${styles.sm}`} onClick={useExample}>
                      usar el ejemplo de Citas Vet
                    </button>
                  </div>
                </div>
              </div>
            </div>
          )}
          <ImportReport report={report} from={from} nextVersion={nextVersion} sealing={sealing} onSeal={seal} onComplete={report?.missingRequired.length ? completeInChat : undefined} />
        </div>
      </div>
      <StepFoot step={3} hint={hint} onBack={onBack} onNext={onNext} nextDisabled={!sealed} />
    </>
  );
}
