import { useState } from 'react';
import { dayTimeline, orderTimeline, pct, type TlModel } from '../../lib/factoryTimeline';
import type { LiveData, Station } from '../../types/factory';
import styles from '../Factory.module.css';

const STATE: Record<string, string> = { done: 'hecho', run: 'en curso', wait: 'en espera por límite', plan: 'estimado', human: 'esperando a una persona' };
const BAR: Record<string, string> = { done: styles.tlDone, run: styles.tlRun, wait: styles.tlWait, plan: styles.tlPlan, human: styles.tlHuman };
const COLOR: Record<Station, string> = { ing: 'var(--acc)', fab: 'var(--teal)', test: 'var(--grn)', rev: 'var(--gemini)', dec: 'var(--amber)', ent: 'var(--mut)' };
const MARK: Record<string, string> = { '': styles.tlMark, gap: `${styles.tlMark} ${styles.tlGap}`, ok: `${styles.tlMark} ${styles.tlOk}`, back: `${styles.tlMark} ${styles.tlBack}` };

const fmt = (ms: number) => {
  const d = new Date(ms);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
};

/**
 * What the Factory is doing, on a time axis. «Esta orden»: one row per agent
 * — done solid, now up to the red line, a limit wait hatched, what's left
 * dotted and estimated, decisions as diamonds with their rule. «Hoy»: one row
 * per order, coloured by station.
 */
export function Timeline({ live, onPickStation }: { live: LiveData; onPickStation: (station: Station) => void }) {
  const [mode, setMode] = useState<'order' | 'day'>('order');
  const model: TlModel | null = mode === 'order' ? orderTimeline(live) : dayTimeline(live);
  const rules = new Map(live.decisions.filter((d) => d.ruleId).map((d) => [d.ruleId!, d.text]));

  const ticks: number[] = [];
  if (model) {
    const step = model.tick * 60_000;
    for (let t = Math.ceil(model.from / step) * step; t <= model.to; t += step) ticks.push(t);
  }

  return (
    <div className={styles.cardFlat}>
      <div className={styles.cardHead}>
        <span className={styles.sectionLabel}>LÍNEA DE TIEMPO</span>
        <span className={styles.hint}>
          {mode === 'order' ? `${live.order?.key ?? ''} · cada fila es un agente · clic en una barra para ver su actividad` : 'Hoy · cada fila es una orden, coloreada por estación'}
        </span>
        <span className={styles.spacer} />
        <span className={styles.tlLegend}>
          {(['ing', 'fab', 'test', 'rev', 'dec', 'ent'] as Station[]).map((s) => (
            <span key={s}>
              <i style={{ background: COLOR[s] }} />
              {{ ing: 'Ingeniería', fab: 'Fabricación', test: 'Testers', rev: 'Revisión', dec: 'Decisión', ent: 'Entrega' }[s]}
            </span>
          ))}
          <span>
            <i className={styles.tlLegendWait} />
            espera
          </span>
          <span>
            <i className={styles.tlLegendPlan} />
            estimado
          </span>
        </span>
        <div className={styles.seg}>
          <button aria-pressed={mode === 'order'} onClick={() => setMode('order')}>
            Esta orden
          </button>
          <button aria-pressed={mode === 'day'} onClick={() => setMode('day')}>
            Hoy
          </button>
        </div>
      </div>
      {!model ? (
        <span className={`${styles.hint} ${styles.cardPadCol}`}>Aún no hay nada que dibujar.</span>
      ) : (
        <div className={styles.tlWrap}>
          {model.rows.map((r, i) => (
            <div key={`${r.label}-${i}`} className={styles.tlRow}>
              <span className={styles.tlLabel}>
                {r.label}
                <span className={styles.tlM}>{r.m}</span>
              </span>
              <span className={styles.tlTrack}>
                {r.bars.map((b, k) => (
                  <span
                    key={k}
                    className={BAR[b.kind]}
                    style={{ left: `${pct(model, b.from)}%`, width: `${Math.max(0.6, pct(model, b.to) - pct(model, b.from))}%`, ['--c' as string]: COLOR[b.station] }}
                    title={`${b.title} · ${fmt(b.from)}–${fmt(b.to)} · ${STATE[b.kind]}`}
                    onClick={() => onPickStation(b.station)}
                  />
                ))}
                {r.marks.map((m, k) => (
                  <span key={`m${k}`}>
                    <span
                      className={MARK[m.cls]}
                      style={{ left: `${pct(model, m.at)}%` }}
                      title={rules.has(m.text) ? `${m.text} · ${rules.get(m.text)}` : m.cls === 'gap' ? 'El brain no lo cubre: decisión conservadora, va a la bandeja de enmiendas' : m.text}
                    />
                    {m.labelled && (
                      <span className={`${styles.tlMarkText} ${m.cls === 'gap' ? styles.warn : m.cls === 'ok' ? styles.ok : m.cls === 'back' ? styles.bad : ''}`} style={{ left: `${pct(model, m.at)}%` }}>
                        {m.text}
                      </span>
                    )}
                  </span>
                ))}
                {r.note && (
                  <span className={styles.tlNote} style={{ left: `${Math.max(0, pct(model, r.note.at))}%` }}>
                    {r.note.text}
                  </span>
                )}
              </span>
            </div>
          ))}
          <div className={styles.tlAxis}>
            {ticks.map((t) => (
              <span key={t} className={styles.tlTick} style={{ left: `${pct(model, t)}%` }}>
                {fmt(t)}
              </span>
            ))}
          </div>
          {model.now >= model.from && model.now <= model.to && (
            <div className={styles.tlNow} style={{ left: `calc(16px + 190px + (100% - 32px - 190px) * ${pct(model, model.now) / 100})` }}>
              <span>ahora {fmt(model.now)}</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}
