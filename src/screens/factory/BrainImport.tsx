import { BRAIN_SECTIONS, levelLabel, sectionName, type BrainReport, type BrainRule } from '../../types/factory';
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
}: {
  report: BrainReport | null;
  from: string;
  nextVersion: number;
  sealing: boolean;
  onSeal: () => void;
  onComplete?: () => void;
}) {
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
  const blocked = report.missingRequired.length > 0;
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
            <b>{report.sections.length} de 11 secciones</b> reconocidas · <b>{report.rules.length} reglas</b> · {report.checks} con check:
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
      </div>
      <BrainMarkdown rules={report.rules} pending={report.pending} highlight={report.assigned} maxHeight={300} />
      <div className={styles.cardActions}>
        <button className={styles.btnPrimary} disabled={blocked || sealing} title={blocked ? 'Faltan secciones obligatorias' : undefined} onClick={onSeal}>
          {sealing ? 'sellando…' : `✋ Sellar brain v${nextVersion}`}
        </button>
        {onComplete && (
          <button className={styles.btn} onClick={onComplete}>
            Completar lo que falta en una conversación
          </button>
        )}
      </div>
    </div>
  );
}
