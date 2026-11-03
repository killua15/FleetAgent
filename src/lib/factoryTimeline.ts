import { pointsLabel, usd, type LiveData, type Station, type StepRow } from '../types/factory';

/** A stretch of a row: done, running now, waiting for a limit, waiting for a
 *  person, or still to come (estimated). */
export interface TlBar {
  from: number;
  to: number;
  kind: 'done' | 'run' | 'wait' | 'plan' | 'human';
  station: Station;
  stepId?: string;
  title: string;
  /** What the step spent, for the tooltip: «≈ +3 % 5 h · $0,42». */
  spent?: string;
}

/** A diamond: a decision with its rule, a gap in the brain, a round back. */
export interface TlMark {
  at: number;
  text: string;
  cls: '' | 'gap' | 'ok' | 'back';
  labelled: boolean;
}

export interface TlRow {
  label: string;
  m: string;
  bars: TlBar[];
  marks: TlMark[];
  note?: { at: number; text: string };
}

export interface TlModel {
  from: number;
  to: number;
  /** Minutes between axis ticks. */
  tick: number;
  now: number;
  rows: TlRow[];
}

const MIN = 60_000;

/** When there's no history yet: minutes a step of each station takes. */
const DEFAULT_MIN: Record<Station, number> = { ing: 6, fab: 15, test: 6, rev: 5, dec: 2, ent: 1 };

const t = (iso: string) => new Date(iso).getTime();

function pickTick(span: number): number {
  const minutes = span / MIN;
  if (minutes <= 90) return 10;
  if (minutes <= 240) return 30;
  return 60;
}

function range(rows: TlRow[], start: number, now: number): Pick<TlModel, 'from' | 'to' | 'tick'> {
  const ends = rows.flatMap((r) => [...r.bars.map((b) => b.to), ...(r.note ? [r.note.at] : [])]);
  const from = start - 2 * MIN;
  const to = Math.max(now, ...ends) + 6 * MIN;
  return { from, to, tick: pickTick(to - from) };
}

/** A step as bars: what it did, then the wait (limit or person) after it. */
function stepBars(s: StepRow, now: number, resumeAt: number | null, title: string): TlBar[] {
  const start = t(s.startedAt);
  const end = s.finishedAt ? t(s.finishedAt) : now;
  const base = { station: s.station, stepId: s.id, title };
  if (s.status === 'running') return [{ ...base, from: start, to: now, kind: 'run' }];
  if (s.status === 'skipped') return [];
  if (s.status === 'human') return [{ ...base, from: end, to: Math.max(end + MIN, now), kind: 'human' }];
  const points = pointsLabel(s.usageSession, s.usageWeekly);
  const spent = points || s.costUsd > 0 ? [points, s.costUsd > 0 ? `${usd(s.costUsd)} equivalentes` : null].filter(Boolean).join(' · ') : undefined;
  const bars: TlBar[] = [{ ...base, from: start, to: Math.max(end, start + 20_000), kind: 'done', spent }];
  if (s.status === 'wait' && resumeAt) bars.push({ ...base, from: end, to: Math.max(resumeAt, end), kind: 'wait' });
  return bars;
}

/**
 * «Esta orden»: one row per agent of the frozen team, in the order of the
 * line. Done in solid colour; now, up to the now line; a limit wait hatched
 * to its reset; what's left dotted, estimated from each station's average.
 */
export function orderTimeline(live: LiveData, nowDate: Date = new Date()): TlModel | null {
  const run = live.run;
  if (!run) return null;
  const now = nowDate.getTime();
  const waitingLimit = run.status === 'waiting_limit' && run.resumeAt ? t(run.resumeAt) : null;
  const touchesUi = live.order?.touchesUi ?? false;
  const avg = (st: Station) => (live.averages[st] ? live.averages[st]! * 1000 : DEFAULT_MIN[st] * MIN);

  // The rows, in line order: agents, with the human and no-AI checkpoints.
  type Slot = { label: string; m: string; station: Station; agentId?: string; match?: string; skip?: string };
  const team = run.team.filter((a) => a.active);
  const by = (role: string) => team.filter((a) => a.role === role);
  const slots: Slot[] = [
    ...by('spec').map((a) => ({ label: a.name, m: a.model, station: 'ing' as Station, agentId: a.id })),
    ...by('critic').map((a) => ({ label: a.name, m: a.model, station: 'ing' as Station, agentId: a.id })),
    { label: '✋ Spec', m: '', station: 'ing', match: '✋ Spec' },
    ...by('builder').map((a) => ({ label: a.name, m: run.state.escalated && a.escalateModel ? a.escalateModel : a.model, station: 'fab' as Station, agentId: a.id })),
    ...by('tester').map((a) => ({ label: a.name, m: a.model, station: 'test' as Station, agentId: a.id })),
    { label: '¿Suite en verde?', m: 'sin IA', station: 'test', match: '¿Suite en verde?' },
    ...[...by('reviewer_code'), ...by('reviewer_security')].map((a) => ({ label: a.name, m: a.model, station: 'rev' as Station, agentId: a.id })),
    ...[...by('reviewer_ui'), ...by('reviewer_ux')].map((a) => ({
      label: a.name,
      m: a.model,
      station: 'rev' as Station,
      agentId: a.id,
      skip: touchesUi ? undefined : 'no aplica · no toca la interfaz',
    })),
    ...by('verifier').map((a) => ({ label: a.name, m: a.model, station: 'dec' as Station, agentId: a.id })),
    ...by('approver').map((a) => ({ label: a.name, m: a.model, station: 'dec' as Station, agentId: a.id })),
    { label: '✋ Entrega', m: '', station: 'dec', match: '✋ Entrega' },
    { label: 'Entrega', m: '', station: 'ent', match: 'Entrega' },
  ];

  const rows: TlRow[] = slots.map((slot) => {
    const steps = live.steps.filter((s) => (slot.agentId ? s.agentId === slot.agentId : s.label === slot.match));
    const bars = steps.flatMap((s) => stepBars(s, now, waitingLimit, `${slot.label}${slot.m ? ` · ${slot.m}` : ''}`));
    const ids = new Set(steps.map((s) => s.id));
    const marks: TlMark[] = live.decisions
      .filter((d) => d.stepId && ids.has(d.stepId))
      .map((d) => ({ at: t(d.at), text: d.ruleId ?? 'sin regla → bandeja', cls: d.ruleId ? '' : 'gap', labelled: !d.ruleId }));
    if (slot.agentId && run.team.find((a) => a.id === slot.agentId)?.role === 'builder') {
      for (const a of live.activity.filter((x) => x.text.startsWith('↺'))) marks.push({ at: t(a.at), text: a.text, cls: 'back', labelled: true });
    }
    const skipped = steps.find((s) => s.status === 'skipped');
    const note = slot.skip ? { at: t(run.startedAt), text: slot.skip } : skipped ? { at: t(skipped.startedAt), text: skipped.note ?? 'saltada' } : undefined;
    return { label: slot.label, m: slot.m, bars, marks, note };
  });

  // What's left, after now (or after the reset), estimated.
  if (run.status === 'running' || run.status === 'waiting_limit') {
    let cursor = Math.max(now, waitingLimit ?? 0);
    const waitRow = rows.findIndex((r) => r.bars.some((b) => b.kind === 'wait'));
    if (waitRow >= 0) {
      // The waiting agent finishes its part after the reset.
      const st = slots[waitRow].station;
      rows[waitRow].bars.push({ from: cursor, to: cursor + avg(st) / 2, kind: 'plan', station: st, title: `${rows[waitRow].label} · estimado` });
      cursor += avg(st) / 2;
    }
    const runRow = rows.findIndex((r) => r.bars.some((b) => b.kind === 'run'));
    if (runRow >= 0) cursor = Math.max(cursor, now + avg(slots[runRow].station) / 2);
    const last = rows.reduce((found, r, k) => (r.bars.length ? k : found), -1);
    let groupStart = cursor;
    rows.forEach((r, i) => {
      const slot = slots[i];
      if (i <= last || slot.skip || slot.match?.startsWith('✋')) return;
      const d = slot.match === 'Entrega' ? MIN : avg(slot.station);
      // Testers and reviewers work side by side: the group starts together.
      const prev = slots[i - 1];
      const sameGroup = i - 1 > last && !!prev && !prev.match && !slot.match && prev.station === slot.station && (slot.station === 'rev' || slot.station === 'test');
      const from = sameGroup ? groupStart : cursor;
      if (!sameGroup) groupStart = cursor;
      r.bars.push({ from, to: from + d, kind: 'plan', station: slot.station, title: `${r.label} · estimado` });
      cursor = Math.max(cursor, from + d);
    });
  }

  // Drop the human checkpoints nobody stopped at.
  const shown = rows.filter((r, i) => !slots[i].match?.startsWith('✋') || r.bars.length || r.note);
  return { ...range(shown, t(run.startedAt), now), now, rows: shown };
}

const STATUS_WORD: Record<string, string> = {
  merged: 'fusionada',
  delivered: 'entregada',
  running: 'en curso',
  waiting_limit: 'en espera',
  waiting_human: '✋ te espera',
  failed: 'fallida',
  stopped: 'parada',
};

/** «Hoy»: one row per order, coloured by station, with the rounds back (↺),
 *  the waits and the delivery. */
export function dayTimeline(live: LiveData, nowDate: Date = new Date()): TlModel | null {
  if (!live.today.length) return null;
  const now = nowDate.getTime();
  const rows: TlRow[] = live.today.map((d) => {
    const resume = d.status === 'waiting_limit' && d.resumeAt ? t(d.resumeAt) : null;
    const bars = d.steps.flatMap((s) => stepBars(s, now, resume, `${d.orderKey} · ${s.label}`));
    const marks: TlMark[] = [];
    let lastRound = 0;
    for (const s of d.steps) {
      if (s.round > lastRound) {
        marks.push({ at: t(s.startedAt), text: `↺ ronda ${s.round}`, cls: 'back', labelled: false });
        lastRound = s.round;
      }
    }
    const end = d.steps.length ? Math.max(...d.steps.map((s) => (s.finishedAt ? t(s.finishedAt) : now))) : now;
    if (d.status === 'merged' || d.status === 'delivered') marks.push({ at: end, text: STATUS_WORD[d.status], cls: 'ok', labelled: true });
    return { label: `${d.orderKey} ${d.title}`, m: STATUS_WORD[d.status] ?? d.status, bars, marks };
  });
  const starts = live.today.flatMap((d) => d.steps.map((s) => t(s.startedAt)));
  return { ...range(rows, Math.min(...(starts.length ? starts : [now])), now), now, rows };
}

/** A time as a percentage of the model's span. */
export const pct = (m: TlModel, at: number) => ((at - m.from) / (m.to - m.from)) * 100;
