import { describe, expect, it } from 'vitest';
import { dayTimeline, orderTimeline } from './factoryTimeline';
import type { FactoryAgent, LiveData, StepRow } from '../types/factory';

const agent = (id: string, role: FactoryAgent['role'], name: string, model: string): FactoryAgent => ({
  id,
  role,
  group: '',
  name,
  short: name,
  duty: '',
  sections: [],
  accountId: 'a',
  model,
  active: true,
  required: false,
  command: role === 'tester' ? 'pytest -q' : null,
  escalateModel: role === 'builder' ? 'opus' : null,
  note: null,
  position: 0,
});

const team = [
  agent('spec', 'spec', 'Spec writer', 'opus'),
  agent('critic', 'critic', 'Crítico R0', 'haiku'),
  agent('builder', 'builder', 'Builder', 'sonnet'),
  agent('tu', 'tester', 'Tester unitario', 'sonnet'),
  agent('ti', 'tester', 'Tester integración', 'sonnet'),
  agent('rc', 'reviewer_code', 'Revisor de código', 'opus'),
  agent('rs', 'reviewer_security', 'Seguridad', 'opus'),
  agent('rui', 'reviewer_ui', 'Revisor UI', 'sonnet'),
  agent('ver', 'verifier', 'Verificador', 'haiku'),
  agent('ap', 'approver', 'Aprobador', 'gpt-5'),
];

const at = (hm: string) => `2026-10-30T${hm}:00`;
const step = (id: string, agentId: string | null, label: string, station: StepRow['station'], status: StepRow['status'], from: string, to: string | null): StepRow => ({
  id,
  runId: 'r',
  agentId,
  label,
  model: '',
  station,
  status,
  round: 0,
  note: status === 'skipped' ? 'saltada · modo autónomo' : null,
  costUsd: 0,
  startedAt: at(from),
  finishedAt: to ? at(to) : null,
});

/** VET-2 at 13:46, as the prototype shows it: spec done, builder waiting for
 *  the 14:00 reset. */
const live: LiveData = {
  run: {
    id: 'r',
    projectId: 'p',
    orderId: 'o',
    status: 'waiting_limit',
    stage: 'build',
    round: 0,
    team,
    brainVersion: 1,
    branch: 'feat/VET-2-cancelar-cita',
    worktree: '',
    base: 'dev',
    resumeAt: at('14:00'),
    mrUrl: null,
    costUsd: 0.94,
    error: null,
    state: { spec: null, feedback: null, findings: [], warnings: [], escalated: false, gateOk: false, gate: [], humanKind: null, human: null, summary: null, waived: [], broken: [] },
    startedAt: at('13:21'),
    finishedAt: null,
  },
  order: null,
  steps: [
    step('s1', 'spec', 'Spec writer', 'ing', 'done', '13:21', '13:27'),
    step('s2', 'critic', 'Crítico R0', 'ing', 'done', '13:27', '13:28'),
    step('s3', null, '✋ Spec', 'ing', 'skipped', '13:28', '13:28'),
    step('s4', 'builder', 'Builder', 'fab', 'wait', '13:28', '13:42'),
  ],
  activity: [],
  decisions: [
    { stepId: 's1', station: 'ing', at: at('13:22'), ruleId: 'ARQ-02', text: 'el handler delega' },
    { stepId: 's1', station: 'ing', at: at('13:26'), ruleId: null, text: 'no se avisa al veterinario' },
  ],
  today: [],
  averages: {},
};

describe('order timeline', () => {
  const m = orderTimeline(live, new Date(at('13:46')))!;
  const row = (label: string) => m.rows.find((r) => r.label === label)!;

  it('shows what each agent did, the limit wait and the decisions with their rule', () => {
    expect(row('Spec writer').bars.map((b) => b.kind)).toEqual(['done']);
    expect(row('Spec writer').marks.map((x) => [x.text, x.cls])).toEqual([
      ['ARQ-02', ''],
      ['sin regla → bandeja', 'gap'],
    ]);
    const builder = row('Builder').bars;
    expect(builder.map((b) => b.kind)).toEqual(['done', 'wait', 'plan']);
    expect(builder[1].to).toBe(new Date(at('14:00')).getTime());
    expect(builder[2].from).toBe(builder[1].to);
  });

  it('estimates the rest after the reset, testers and reviewers side by side', () => {
    const [unit, integ] = [row('Tester unitario').bars[0], row('Tester integración').bars[0]];
    expect(unit.kind).toBe('plan');
    expect(unit.from).toBe(integ.from);
    expect(unit.from).toBeGreaterThanOrEqual(new Date(at('14:00')).getTime());
    expect(row('¿Suite en verde?').bars[0].from).toBeGreaterThanOrEqual(unit.to);
    expect(row('Revisor de código').bars[0].from).toBe(row('Seguridad').bars[0].from);
    expect(row('Revisor UI').note?.text).toBe('no aplica · no toca la interfaz');
    expect(row('Revisor UI').bars).toEqual([]);
    expect(row('✋ Spec').note?.text).toBe('saltada · modo autónomo');
    expect(m.rows.some((r) => r.label === '✋ Entrega')).toBe(false);
  });
});

describe('day timeline', () => {
  it('is one row per order, with the rounds back and the merge', () => {
    const day = dayTimeline(
      {
        ...live,
        today: [
          {
            orderKey: 'VET-1',
            title: 'Reservar cita',
            status: 'merged',
            resumeAt: null,
            steps: [step('a', 'builder', 'Builder', 'fab', 'done', '10:09', '10:31'), { ...step('b', 'builder', 'Builder', 'fab', 'done', '10:46', '10:52'), round: 1 }],
          },
        ],
      },
      new Date(at('13:46')),
    )!;
    expect(day.rows[0].label).toBe('VET-1 Reservar cita');
    expect(day.rows[0].marks.map((x) => x.cls)).toEqual(['back', 'ok']);
    expect(day.tick).toBe(30);
  });
});
