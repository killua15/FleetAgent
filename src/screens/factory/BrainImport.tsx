import { useEffect, useState } from 'react';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import { BRAIN_SECTIONS, LINT_KIND, levelLabel, sectionName, type BrainReport, type BrainRule, type LintFinding } from '../../types/factory';
import { sendToBrainSession } from './BrainSession';
import styles from '../Factory.module.css';

/** The sealed shape of a brain, coloured like the prototype's `.brainMd`. */
export function BrainMarkdown({ rules, pending, highlight, maxHeight }: { rules: BrainRule[]; pending?: string[]; highlight?: string[]; maxHeight?: number }) {
  const marked = new Set(highlight ?? []);
  return (
    <pre className={styles.brainMd} style={maxHeight ? { maxHeight } : undefined}>
      {BRAIN_SECTIONS.map(([key, name]) => {
        const inSection = rules.filter((r) => r.section === key);
        if (!inSection.length) return null;
        return (
          <span key={key}>
            {'\n'}
            <span className={styles.mdH}>## {name}</span>
            {'\n'}
            {inSection.map((r) => (
              <span key={r.id} className={marked.has(r.id) ? styles.mdNew : undefined}>
                {'- '}
                <span className={styles.mdId}>{r.id}</span>
                {' · '}
                <span className={r.level === 'must' ? styles.mdMust : styles.mdShould}>{levelLabel(r.level)}</span>
                {` · ${r.text}\n`}
                {r.check && <span className={styles.mdChk}>{`  check: ${r.check}\n`}</span>}
              </span>
            ))}
          </span>
        );
      })}
      {pending && pending.length > 0 && (
        <span>
          {'\n'}
          <span className={styles.mdH}>## Pendiente</span>
          {'\n'}
          {pending.map((p) => `- ${p}\n`).join('')}
        </span>
      )}
    </pre>
  );
}

/** «Lo que se importó»: what the parser recognised, what's missing and what to
 *  look at before sealing — the same report for all three ways in. */
export function ImportReport({
  report,
  from,
  nextVersion,
  sealing,
  onSeal,
  onComplete,
  projectId,
}: {
  report: BrainReport | null;
  from: string;
  nextVersion: number;
  sealing: boolean;
  onSeal: () => void;
  onComplete?: () => void;
  /** To run the brain's own checks against this project and machine. */
  projectId?: string;
}) {
  const [findings, setFindings] = useState<LintFinding[] | null>(null);
  const markdown = report?.markdown;
  useEffect(() => {
    setFindings(null);
    if (!projectId || !markdown) return;
    let live = true;
    invoke<LintFinding[]>('factory_brain_lint', { projectId, markdown })
      .then((f) => live && setFindings(f))
      .catch(() => live && setFindings([]));
    return () => {
      live = false;
    };
  }, [projectId, markdown]);

  if (!report) {
    return (
      <div className={styles.cardFlat}>
        <div className={styles.cardHead}>
          <span className={styles.sectionLabel}>LO QUE SE IMPORTÓ</span>
          <span className={styles.spacer} />
          <span className={styles.num}>nada todavía</span>
        </div>
        <div className={styles.report}>
          <span className={styles.hint}>
            Cuando importes el archivo, aquí verás las secciones y reglas que se reconocieron, lo que falta y lo que hay que revisar antes de sellar.
          </span>
        </div>
      </div>
    );
  }
  const missing = report.missing.map(sectionName);
  const errors = findings?.filter((f) => f.severity === 'error') ?? [];
  const checking = !!projectId && findings === null;
  const blocked = report.missingRequired.length > 0 || errors.length > 0 || checking;
  const fixInSession = () => {
    const list = (findings ?? []).map((f) => `${f.ruleId || 'brain'}: ${f.message.split('\n')[0]} (${f.fix})`).join(' · ');
    const sent = sendToBrainSession(projectId!, `Hangar ha comprobado el brain y no se puede sellar así. Corrige esto, ejecuta cada check aquí para comprobarlo y vuelve a generar el BRAIN.md: ${list}`);
    if (sent) toastSuccess('Enviado a la sesión del brain');
    else toastError('No hay una sesión del brain abierta: ábrela (Conversación aquí o Sesión de enmienda) y vuelve a pulsar');
  };
  return (
    <div className={styles.cardFlat}>
      <div className={styles.cardHead}>
        <span className={styles.sectionLabel}>LO QUE SE IMPORTÓ</span>
        <span className={styles.spacer} />
        <span className={styles.num}>
          {from} · {report.rules.length} reglas
        </span>
      </div>
      <div className={styles.report}>
        <div className={styles.repLine}>
          <span className={styles.ok}>✓</span>
          <span>
            <b>
              {report.sections.length} de {BRAIN_SECTIONS.length} secciones
            </b> reconocidas · <b>{report.rules.length} reglas</b> · {report.checks} con check:
          </span>
        </div>
        {report.assigned.length > 0 && (
          <div className={styles.repLine}>
            <span className={styles.warn}>⚠</span>
            <span>
              {report.assigned.length} reglas sin ID: se asignaron <span className={styles.mono}>{report.assigned.join(', ')}</span>
            </span>
          </div>
        )}
        {report.pending.length > 0 && (
          <div className={styles.repLine}>
            <span className={styles.accTxt}>?</span>
            <span>
              {report.pending.length} pendiente(s): «{report.pending[0]}» → se resuelven en una conversación corta o van a la bandeja de enmiendas
            </span>
          </div>
        )}
        {missing.length > 0 ? (
          <div className={styles.repLine}>
            <span className={blocked ? styles.bad : styles.dimTxt}>{blocked ? '✗' : '○'}</span>
            <span>
              Faltan: {missing.join(', ')}
              {blocked ? (
                <>
                  {' · '}
                  <b>obligatorias para sellar</b>
                </>
              ) : (
                ' (opcionales)'
              )}
            </span>
          </div>
        ) : (
          <div className={styles.repLine}>
            <span className={styles.ok}>✓</span>
            <span>Están todas las secciones.</span>
          </div>
        )}
        {report.unknown.length > 0 && (
          <div className={styles.repLine}>
            <span className={styles.dimTxt}>○</span>
            <span>Ignorado: {report.unknown.join(', ')}</span>
          </div>
        )}
        {checking && (
          <div className={styles.repLine}>
            <span className={styles.dimTxt}>…</span>
            <span>Comprobando los checks en esta máquina…</span>
          </div>
        )}
        {findings && findings.length === 0 && (
          <div className={styles.repLine}>
            <span className={styles.ok}>✓</span>
            <span>Cada check se puede ejecutar, no cae en trampas conocidas, encaja con la Factory y la máquina cumple el Entorno.</span>
          </div>
        )}
      </div>
      {findings && findings.length > 0 && (
        <details className={`${styles.acc} ${styles.lint}`} open={errors.length > 0}>
          <summary className={styles.accHead}>
            <span className={styles.sectionLabel}>COMPROBACIONES DEL BRAIN</span>
            {errors.length > 0 && <span className={styles.chipRed}>{errors.length} bloquean el sello</span>}
            {findings.length > errors.length && <span className={styles.chipAmber}>{findings.length - errors.length} avisos</span>}
          </summary>
          <div className={styles.gateList}>
            {findings.map((f, i) => (
              <div key={i} className={styles.lintRow}>
                <span className={f.severity === 'error' ? styles.bad : styles.warn}>{f.severity === 'error' ? '✗' : '⚠'}</span>
                <span className={styles.ruleId}>{f.ruleId || '—'}</span>
                <div>
                  <span className={styles.chip}>{LINT_KIND[f.kind]}</span> <span className={styles.ruleTxt}>{f.message.split('\n')[0]}</span>
                  {f.message.includes('\n') && <pre className={styles.gateOut}>{f.message.split('\n').slice(1).join('\n')}</pre>}
                  <div className={styles.hint}>→ {f.fix}</div>
                </div>
              </div>
            ))}
          </div>
        </details>
      )}
      <BrainMarkdown rules={report.rules} pending={report.pending} highlight={report.assigned} maxHeight={300} />
      <div className={styles.cardActions}>
        <button
          className={styles.btnPrimary}
          disabled={blocked || sealing}
          title={report.missingRequired.length ? 'Faltan secciones obligatorias' : errors.length ? 'El brain no pasa sus comprobaciones' : checking ? 'Comprobando…' : undefined}
          onClick={onSeal}
        >
          {sealing ? 'sellando…' : `✋ Sellar brain v${nextVersion}`}
        </button>
        {findings && findings.length > 0 && projectId && (
          <button className={styles.btn} title="Le pasa estos hallazgos a la sesión del brain abierta" onClick={fixInSession}>
            Arreglar en la sesión
          </button>
        )}
        {onComplete && (
          <button className={styles.btn} onClick={onComplete}>
            Completar lo que falta en una conversación
          </button>
        )}
      </div>
    </div>
  );
}
