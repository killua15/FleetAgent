import { useCallback, useEffect, useState } from 'react';
import { save as saveDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import { BRAIN_SECTIONS, levelLabel, type ActiveCheck, type Amendment, type BrainReport, type BrainRule, type FactoryProject, type SealedBrain } from '../../types/factory';
import { ImportReport } from './BrainImport';
import { BrainSession } from './BrainSession';
import { DecisionsEditor } from './DecisionsEditor';
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
  // Rules edited by hand (✎, 🗑), until sealed as the next version.
  const [draft, setDraft] = useState<BrainRule[] | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [fromEdit, setFromEdit] = useState(false);
  const [deciding, setDeciding] = useState(false);
  const [fromDecisions, setFromDecisions] = useState(false);

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

  const seal = async (active?: ActiveCheck[]) => {
    if (!report) return;
    setSealing(true);
    try {
      const next = await invoke<SealedBrain>('factory_brain_seal', { projectId: project.projectId, markdown: report.markdown, source: fromDecisions ? 'decisions' : fromEdit ? 'edit' : 'amendment', active: active ?? null });
      if (fromDecisions) await invoke('factory_decisions_sealed', { projectId: project.projectId });
      toastSuccess(`Brain v${next.version} sellado · las órdenes nuevas lo congelan; las que están en curso siguen con la suya`);
      setReport(null);
      setAmending(false);
      setDraft(null);
      setFromEdit(false);
      setFromDecisions(false);
      setDeciding(false);
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
  const current = draft ?? brain.rules;
  const rules = current.filter((r) => r.section === section);
  const changes = draft ? countChanges(brain.rules, draft) : 0;

  const saveRule = (rule: BrainRule) => {
    setDraft(current.map((r) => (r.id === rule.id ? rule : r)));
    setEditing(null);
  };
  const deleteRule = (id: string) => setDraft(current.filter((r) => r.id !== id));
  const reviewDraft = async () => {
    if (!draft) return;
    try {
      setReport(await invoke<BrainReport>('factory_brain_from_rules', { projectId: project.projectId, rules: draft }));
      setFromEdit(true);
      setAmending(false);
    } catch (e) {
      toastError(String(e));
    }
  };

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
        <button className={`${deciding ? styles.btnPrimary : styles.btn} ${styles.sm}`} onClick={() => setDeciding(!deciding)}>
          {deciding ? 'Cerrar las decisiones' : 'Decisiones'}
        </button>
        <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={() => setAmending(!amending)}>
          {amending ? 'Cerrar la enmienda' : `Sesión de enmienda → v${brain.version + 1}`}
        </button>
      </div>

      <Legend />

      {deciding && (
        <DecisionsEditor
          projectId={project.projectId}
          onReview={(r) => {
            setReport(r);
            setFromDecisions(true);
            setFromEdit(false);
            setAmending(false);
          }}
        />
      )}
      {fromDecisions && report && (
        <ImportReport report={report} from="decisiones" nextVersion={brain.version + 1} sealing={sealing} onSeal={seal} projectId={project.projectId} />
      )}

      {draft && changes > 0 && (
        <div className={styles.bannerAmber}>
          <span>✎</span>
          <span>
            <b>
              {changes} {changes === 1 ? 'cambio' : 'cambios'} sin sellar
            </b>{' '}
            · v{brain.version} sigue siendo la que usa la línea hasta que selles.
          </span>
          <span className={styles.spacer} />
          <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={reviewDraft}>
            Revisar y sellar v{brain.version + 1}
          </button>
          <button
            className={`${styles.btnGhost} ${styles.sm}`}
            onClick={() => {
              setDraft(null);
              setEditing(null);
              if (fromEdit) {
                setReport(null);
                setFromEdit(false);
              }
            }}
          >
            Descartar
          </button>
        </div>
      )}

      {fromEdit && report && (
        <ImportReport report={report} from="edición a mano" nextVersion={brain.version + 1} sealing={sealing} onSeal={seal} projectId={project.projectId} />
      )}

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
          <ImportReport report={report} from="enmienda" nextVersion={brain.version + 1} sealing={sealing} onSeal={seal} projectId={project.projectId} />
        </div>
      )}

      <div className={styles.brainGrid}>
        <div className={`${styles.cardFlat} ${styles.secList}`}>
          <span className={styles.secGroup}>Del inicio…</span>
          {BRAIN_SECTIONS.map(([key, name]) => (
            <span key={key} className={styles.col} style={{ gap: 0 }}>
              {key === 'cal' && <span className={styles.secGroup}>…a producción</span>}
              <button className={key === section ? styles.secOn : styles.sec} onClick={() => setSection(key)}>
                {name} <span className={styles.num}>{current.filter((r) => r.section === key).length}</span>
              </button>
            </span>
          ))}
        </div>
        <div className={styles.col} style={{ gap: 16 }}>
          <div className={styles.cardFlat}>
            {rules.length === 0 && <span className={`${styles.hint} ${styles.cardPadCol}`}>Esta sección no tiene reglas.</span>}
            {rules.map((r) =>
              editing === r.id ? (
                <RuleEditor key={r.id} rule={r} onSave={saveRule} onCancel={() => setEditing(null)} />
              ) : (
                <div key={r.id} className={`${styles.ruleRow} ${styles.ruleEditable}`}>
                  <span className={styles.ruleId}>{r.id}</span>
                  <span className={r.level === 'must' ? styles.lvlMust : styles.lvlShould}>{levelLabel(r.level)}</span>
                  <div>
                    <div className={styles.ruleTxt}>{r.text}</div>
                    {r.check && <div className={styles.check}>check: {r.check}</div>}
                  </div>
                  <span className={styles.ruleTools}>
                    <button className={`${styles.btnGhost} ${styles.sm}`} title="Editar el nivel, el texto o el check" onClick={() => setEditing(r.id)}>
                      ✎
                    </button>
                    <button className={`${styles.btnGhost} ${styles.sm}`} title="Quitar la regla (se aplica al sellar la versión siguiente)" onClick={() => deleteRule(r.id)}>
                      🗑
                    </button>
                  </span>
                </div>
              ),
            )}
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

/** How many rules a hand edit changed or removed. */
function countChanges(before: BrainRule[], after: BrainRule[]): number {
  const now = new Map(after.map((r) => [r.id, r]));
  return before.filter((r) => {
    const n = now.get(r.id);
    return !n || n.text !== r.text || n.level !== r.level || (n.check ?? '') !== (r.check ?? '');
  }).length;
}

function RuleEditor({ rule, onSave, onCancel }: { rule: BrainRule; onSave: (rule: BrainRule) => void; onCancel: () => void }) {
  const [level, setLevel] = useState(rule.level);
  const [text, setText] = useState(rule.text);
  const [check, setCheck] = useState(rule.check ?? '');
  return (
    <div className={`${styles.ruleRow} ${styles.ruleEditing}`}>
      <span className={styles.ruleId}>{rule.id}</span>
      <select className={styles.select} value={level} onChange={(e) => setLevel(e.target.value as BrainRule['level'])}>
        <option value="must">DEBE</option>
        <option value="should">DEBERÍA</option>
      </select>
      <div className={styles.col} style={{ gap: 6 }}>
        <textarea className={styles.input} rows={2} value={text} onChange={(e) => setText(e.target.value)} />
        <input className={`${styles.input} ${styles.mono}`} placeholder="check: comando que la comprueba (opcional)" value={check} onChange={(e) => setCheck(e.target.value)} />
        <span className={styles.row}>
          <button className={`${styles.btnPrimary} ${styles.sm}`} disabled={!text.trim()} onClick={() => onSave({ ...rule, level, text: text.trim(), check: check.trim() || null })}>
            Guardar en el borrador
          </button>
          <button className={`${styles.btnGhost} ${styles.sm}`} onClick={onCancel}>
            Cancelar
          </button>
        </span>
      </div>
    </div>
  );
}

const LEGEND_SECTIONS: [string, string, string][] = [
  ['PRD', 'Producto', 'problema, usuarios, alcance, glosario'],
  ['ARQ', 'Arquitectura', 'módulos, capas, lo que no se toca'],
  ['TEC', 'Tecnologías', 'stack, versiones, librerías permitidas'],
  ['ENT', 'Entorno', 'lo que esta máquina tiene que tener: runtimes, navegadores, servicios'],
  ['COD', 'Código', 'estilo, nombres, errores, cómo se compila'],
  ['DAT', 'Datos', 'modelo, migraciones, compatibilidad'],
  ['SEC', 'Seguridad', 'secretos, autenticación, datos sensibles'],
  ['CAL', 'Calidad', 'tests, lint, qué significa «hecho»'],
  ['GIT', 'Git y entrega', 'ramas, commits, merge'],
  ['DEP', 'Despliegue', 'entornos, CI/CD, rollback'],
  ['RSG', 'Riesgo', 'qué obliga a parar y preguntar; tamaño máximo'],
  ['DEC', 'Decisiones', 'qué se decidió, cuándo y por qué'],
];

const REQUIRED_PREFIXES = new Set(['PRD', 'ARQ', 'TEC', 'COD', 'CAL', 'GIT', 'RSG']);

/** What the brain's names and marks mean. */
function Legend() {
  return (
    <details className={`${styles.cardFlat} ${styles.acc}`}>
      <summary className={styles.accHead}>
        <span className={styles.sectionLabel}>LEYENDA</span>
        <span className={styles.hint}>qué significa PRD-01, DEBE, check:…</span>
      </summary>
      <div className={styles.legend}>
        <div className={styles.legendGrid}>
          {LEGEND_SECTIONS.map(([p, name, what]) => (
            <span key={p} className={styles.legendItem}>
              <span className={styles.ruleId}>{p}</span>
              <span>
                <b>{name}</b>
                {REQUIRED_PREFIXES.has(p) && <span className={styles.accTxt}> · obligatoria</span>}
                <span className={styles.hint}> — {what}</span>
              </span>
            </span>
          ))}
        </div>
        <div className={styles.legendNotes}>
          <span>
            <span className={styles.ruleId}>PRD-01</span> El prefijo es la sección y el número, su orden. Las decisiones de los agentes citan este ID.
          </span>
          <span>
            <span className={styles.lvlMust}>DEBE</span> Obligatoria: si no se cumple, la orden vuelve a Fabricación.
          </span>
          <span>
            <span className={styles.lvlShould}>DEBERÍA</span> Recomendación: un revisor puede dejarla pasar con un aviso en la entrega.
          </span>
          <span>
            <span className={styles.check}>check:</span> Comando que la puerta de tests ejecuta sin IA en el worktree de la orden. Sale con 0 si la regla se cumple.
          </span>
          <span>
            <b>vN · sellado</b> Cada versión es inmutable. Las órdenes congelan la suya al empezar; editar o enmendar sella la siguiente.
          </span>
          <span>
            <b>Sin regla → bandeja</b> Lo que un agente decide sin una regla que lo cubra va a la bandeja de enmiendas, para resolverlo en la siguiente versión.
          </span>
          <span>
            <b>obligatoria</b> Sección sin la que el brain no se puede sellar.
          </span>
        </div>
      </div>
    </details>
  );
}
