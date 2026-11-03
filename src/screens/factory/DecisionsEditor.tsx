import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke } from '../../lib/invoke';
import { toastError, toastSuccess } from '../../components/ErrorOverlay';
import { BRAIN_SECTIONS, type BrainReport, type DecisionField, type DecisionItem, type DecisionsView } from '../../types/factory';
import styles from '../Factory.module.css';

const BY: Record<DecisionItem['by'], string> = {
  ai: 'propuesta de la IA',
  creator: 'decidido por ti',
  brain: 'del brain sellado',
  inbox: 'de la bandeja',
  factory: 'lo fija la Factory',
};

const newId = () => (crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`);

/**
 * The brain as the creator's decisions: fixed fields per section and free
 * suggestions. The AI reads the repo and proposes; only what the creator
 * confirms goes into the brain, and the Factory writes the check of every
 * field that can be checked.
 */
export function DecisionsEditor({ projectId, onReview }: { projectId: string; onReview: (report: BrainReport) => void }) {
  const [view, setView] = useState<DecisionsView | null>(null);
  const [section, setSection] = useState('prd');
  const [asking, setAsking] = useState(false);
  const [adding, setAdding] = useState<{ text: string; level: 'must' | 'should'; check: string } | null>(null);
  const saveTimer = useRef<number | null>(null);

  useEffect(() => {
    invoke<DecisionsView>('factory_decisions_get', { projectId })
      .then(setView)
      .catch((e) => toastError(String(e)));
  }, [projectId]);

  const persist = useCallback(
    (items: DecisionItem[]) => {
      setView((v) => (v ? { ...v, items } : v));
      if (saveTimer.current) window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => {
        invoke<DecisionsView>('factory_decisions_save', { projectId, items: items.map(strip) })
          .then(setView)
          .catch((e) => toastError(String(e)));
      }, 400);
    },
    [projectId],
  );

  if (!view) return <span className={styles.hint}>cargando las decisiones…</span>;

  const items = view.items;
  const update = (id: string, patch: Partial<DecisionItem>) => persist(items.map((i) => (i.id === id ? { ...i, ...patch } : i)));
  const remove = (id: string) => persist(items.filter((i) => i.id !== id));
  const add = (item: Omit<DecisionItem, 'id'>) => persist([...items, { ...item, id: newId() }]);

  const ask = async () => {
    setAsking(true);
    try {
      setView(await invoke<DecisionsView>('factory_decisions_propose', { projectId }));
      toastSuccess('Propuestas listas: acepta, cambia o descarta cada una');
    } catch (e) {
      toastError(String(e));
    } finally {
      setAsking(false);
    }
  };
  const cancel = () => invoke('factory_decisions_cancel', { projectId }).catch(() => undefined);
  const reset = async () => {
    try {
      setView(await invoke<DecisionsView>('factory_decisions_reset', { projectId }));
    } catch (e) {
      toastError(String(e));
    }
  };
  const review = async () => {
    try {
      if (saveTimer.current) {
        window.clearTimeout(saveTimer.current);
        await invoke('factory_decisions_save', { projectId, items: items.map(strip) });
      }
      onReview(await invoke<BrainReport>('factory_decisions_render', { projectId }));
    } catch (e) {
      toastError(String(e));
    }
  };

  const confirmed = items.filter((i) => i.status === 'confirmed');
  const proposed = items.filter((i) => i.status === 'proposed');
  const checks = confirmed.filter((i) => i.guard).length;
  const inSection = (s: string) => items.filter((i) => i.section === s);
  const fields = view.fields.filter((f) => f.section === section);
  const free = inSection(section).filter((i) => !i.field && i.status !== 'rejected');

  return (
    <div className={`${styles.cardFlat} ${styles.decide}`}>
      <div className={styles.cardHead}>
        <span className={styles.sectionLabel}>DECISIONES DEL PROYECTO</span>
        <span className={styles.chip}>{confirmed.length} decididas</span>
        {proposed.length > 0 && <span className={styles.chipAmber}>{proposed.length} propuestas por revisar</span>}
        <span className={styles.chip}>{checks} con check automático</span>
        <span className={styles.spacer} />
        {asking ? (
          <>
            <span className={styles.hint}>la IA está leyendo el repo…</span>
            <button className={`${styles.btnGhost} ${styles.sm}`} onClick={cancel}>
              Cancelar
            </button>
          </>
        ) : (
          <button className={`${styles.btn} ${styles.sm}`} title="Lee el repo y propone valores para lo que esté vacío, y algunas decisiones más. Tú decides." onClick={ask}>
            ✦ Pedir propuestas a la IA
          </button>
        )}
        <button className={`${styles.btnPrimary} ${styles.sm}`} disabled={!confirmed.length} onClick={review}>
          Revisar y sellar
        </button>
      </div>
      {view.stale && (
        <div className={styles.bannerAmber} style={{ margin: '0 16px 10px' }}>
          <span>⚠</span>
          <span>
            El brain se selló por otro camino (v{view.brainVersion}) después de estas decisiones. Si sellas desde aquí, esas reglas se pierden.
          </span>
          <span className={styles.spacer} />
          <button className={`${styles.btn} ${styles.sm}`} onClick={reset}>
            Partir del brain vigente
          </button>
        </div>
      )}
      <div className={styles.decideGrid}>
        <nav className={styles.secList}>
          {BRAIN_SECTIONS.map(([key, name]) => {
            const n = inSection(key).filter((i) => i.status === 'confirmed').length;
            const p = inSection(key).filter((i) => i.status === 'proposed').length;
            return (
              <button key={key} className={key === section ? styles.secOn : styles.sec} onClick={() => setSection(key)}>
                {name} <span className={styles.num}>{n}</span>
                {p > 0 && <span className={styles.warn}> ·{p}</span>}
              </button>
            );
          })}
        </nav>
        <div className={styles.col} style={{ gap: 10, minWidth: 0 }}>
          {fields.map((f) => (
            <FieldRow key={f.key} def={f} items={items.filter((i) => i.field === f.key)} onUpdate={update} onRemove={remove} onAdd={add} />
          ))}
          <div className={styles.decideFree}>
            <div className={styles.row}>
              <span className={styles.lName}>Más decisiones y sugerencias</span>
              <span className={styles.hint}>lo que estas casillas no cubren: tuyo, o propuesto por la IA</span>
              <span className={styles.spacer} />
              {!adding && (
                <button className={`${styles.btn} ${styles.sm}`} onClick={() => setAdding({ text: '', level: 'should', check: '' })}>
                  + Sugerir
                </button>
              )}
            </div>
            {free.map((i) => (
              <FreeRow key={i.id} item={i} onUpdate={update} onRemove={remove} />
            ))}
            {!free.length && !adding && <span className={styles.hint}>Nada todavía en esta sección.</span>}
            {adding && (
              <div className={styles.decideAdd}>
                <textarea className={styles.input} rows={2} placeholder="La decisión, concreta (p. ej. «Los textos de la interfaz viven en src/i18n/es.ts»)" value={adding.text} onChange={(e) => setAdding({ ...adding, text: e.target.value })} />
                <input className={`${styles.input} ${styles.mono}`} placeholder="check (opcional): comando rápido y local que la comprueba" value={adding.check} onChange={(e) => setAdding({ ...adding, check: e.target.value })} />
                <div className={styles.row}>
                  <Level value={adding.level} onChange={(level) => setAdding({ ...adding, level })} />
                  <span className={styles.spacer} />
                  <button className={`${styles.btnGhost} ${styles.sm}`} onClick={() => setAdding(null)}>
                    Cancelar
                  </button>
                  <button
                    className={`${styles.btnPrimary} ${styles.sm}`}
                    disabled={!adding.text.trim()}
                    onClick={() => {
                      add({ section, field: null, value: adding.text.trim(), level: adding.level, check: adding.check.trim() || null, status: 'confirmed', by: 'creator', why: null, ruleId: null });
                      setAdding(null);
                    }}
                  >
                    Añadir
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

/** What the backend reads back: an item without its computed check. */
function strip({ guard: _g, ...item }: DecisionItem): DecisionItem {
  return item;
}

function Level({ value, onChange }: { value: 'must' | 'should'; onChange: (v: 'must' | 'should') => void }) {
  return (
    <div className={styles.seg}>
      <button aria-pressed={value === 'must'} title="Innegociable: si no se cumple, la orden no se entrega" onClick={() => onChange('must')}>
        DEBE
      </button>
      <button aria-pressed={value === 'should'} title="Recomendación: avisa en la entrega sin bloquear" onClick={() => onChange('should')}>
        DEBERÍA
      </button>
    </div>
  );
}

function ValueInput({ def, value, onChange }: { def: DecisionField; value: string; onChange: (v: string) => void }) {
  if (def.kind === 'choice') {
    return (
      <select className={styles.select} value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">— elige —</option>
        {def.options.map((o) => (
          <option key={o} value={o}>
            {o}
          </option>
        ))}
      </select>
    );
  }
  if (def.kind === 'list') {
    return <textarea className={styles.input} rows={Math.min(6, Math.max(2, value.split('\n').length))} placeholder={def.help} value={value} onChange={(e) => onChange(e.target.value)} />;
  }
  return <input className={styles.input} type={def.kind === 'number' ? 'number' : 'text'} placeholder={def.help} value={value} onChange={(e) => onChange(e.target.value)} />;
}

function FieldRow({
  def,
  items,
  onUpdate,
  onRemove,
  onAdd,
}: {
  def: DecisionField;
  items: DecisionItem[];
  onUpdate: (id: string, patch: Partial<DecisionItem>) => void;
  onRemove: (id: string) => void;
  onAdd: (item: Omit<DecisionItem, 'id'>) => void;
}) {
  const decided = items.find((i) => i.status === 'confirmed');
  const proposal = items.find((i) => i.status === 'proposed');
  return (
    <div className={styles.decideRow}>
      <div className={styles.decideHead}>
        <span className={styles.lName}>{def.label}</span>
        {def.guarded && <span className={styles.chipAcc} title="La Factory escribe el check de esta decisión">check automático</span>}
        {decided && <span className={styles.hint}>· {BY[decided.by]}</span>}
        <span className={styles.spacer} />
        {decided && !def.fixed && <Level value={decided.level} onChange={(level) => onUpdate(decided.id, { level })} />}
      </div>
      {def.help && !def.fixed && <span className={styles.hint}>{def.help}</span>}
      {def.fixed ? (
        <span className={styles.mono}>{def.fixed}</span>
      ) : decided ? (
        <div className={styles.row} style={{ alignItems: 'flex-start' }}>
          <div style={{ flex: 1, minWidth: 0 }}>
            <ValueInput def={def} value={decided.value} onChange={(v) => onUpdate(decided.id, { value: v, by: 'creator' })} />
          </div>
          <button className={`${styles.btnGhost} ${styles.sm}`} title="Quitar esta decisión" onClick={() => onRemove(decided.id)}>
            🗑
          </button>
        </div>
      ) : proposal ? (
        <div className={styles.decideProposal}>
          <div className={styles.mono} style={{ whiteSpace: 'pre-wrap' }}>
            {proposal.value}
          </div>
          {proposal.why && <span className={styles.hint}>la IA: {proposal.why}</span>}
          <div className={styles.row}>
            <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={() => onUpdate(proposal.id, { status: 'confirmed' })}>
              Aceptar
            </button>
            <button className={`${styles.btnGhost} ${styles.sm}`} title="No se volverá a proponer" onClick={() => onUpdate(proposal.id, { status: 'rejected' })}>
              Descartar
            </button>
          </div>
        </div>
      ) : (
        <EmptyField def={def} onDecide={(v) => onAdd({ section: def.section, field: def.key, value: v, level: def.level, check: null, status: 'confirmed', by: 'creator', why: null, ruleId: null })} />
      )}
      {decided?.guard && <code className={styles.decideGuard}>check: {decided.guard}</code>}
    </div>
  );
}

/** An empty field: decided when you leave it (or press Enter), not on the
 *  first key — the input would otherwise turn into another under the cursor. */
function EmptyField({ def, onDecide }: { def: DecisionField; onDecide: (value: string) => void }) {
  const [value, setValue] = useState('');
  if (def.kind === 'choice') return <ValueInput def={def} value="" onChange={(v) => v && onDecide(v)} />;
  const commit = () => value.trim() && onDecide(value.trim());
  return (
    <div
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter' && (def.kind !== 'list' || e.metaKey)) {
          e.preventDefault();
          commit();
        }
      }}
    >
      <ValueInput def={def} value={value} onChange={setValue} />
    </div>
  );
}

function FreeRow({ item, onUpdate, onRemove }: { item: DecisionItem; onUpdate: (id: string, patch: Partial<DecisionItem>) => void; onRemove: (id: string) => void }) {
  if (item.status === 'proposed') {
    return (
      <div className={styles.decideProposal}>
        <span>{item.value}</span>
        {item.why && <span className={styles.hint}>la IA: {item.why}</span>}
        <div className={styles.row}>
          <Level value={item.level} onChange={(level) => onUpdate(item.id, { level })} />
          <span className={styles.spacer} />
          <button className={`${styles.btnPrimary} ${styles.sm}`} onClick={() => onUpdate(item.id, { status: 'confirmed' })}>
            Aceptar
          </button>
          <button className={`${styles.btnGhost} ${styles.sm}`} onClick={() => onUpdate(item.id, { status: 'rejected' })}>
            Descartar
          </button>
        </div>
      </div>
    );
  }
  return (
    <div className={styles.decideRow}>
      <div className={styles.decideHead}>
        {item.ruleId && <span className={styles.ruleId}>{item.ruleId}</span>}
        <span className={styles.hint}>{BY[item.by]}</span>
        <span className={styles.spacer} />
        <Level value={item.level} onChange={(level) => onUpdate(item.id, { level })} />
        <button className={`${styles.btnGhost} ${styles.sm}`} title="Quitar" onClick={() => onRemove(item.id)}>
          🗑
        </button>
      </div>
      <textarea className={styles.input} rows={2} value={item.value} onChange={(e) => onUpdate(item.id, { value: e.target.value })} />
      <input className={`${styles.input} ${styles.mono}`} placeholder="check (opcional)" value={item.check ?? ''} onChange={(e) => onUpdate(item.id, { check: e.target.value || null })} />
    </div>
  );
}
