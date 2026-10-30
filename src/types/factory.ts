/** Where a Factory project's tasks come from. `none`: they live in Hangar. */
export type FactoryProvider = 'clickup' | 'github' | 'none';
/** `local`: merged into the local base branch, never pushed. */
export type FactoryDelivery = 'mr' | 'local';
export type FactoryMode = 'supervised' | 'delegated' | 'autonomous';
export type FactoryLoopState = 'stopped' | 'running' | 'paused';

/** A project that joined the Factory, as `factory::store::FactoryProject`. */
export interface FactoryProject {
  projectId: string;
  projectName: string;
  projectPath: string;
  providerKind: FactoryProvider;
  providerSourceId: string | null;
  /** A ClickUp list id, or a GitHub `owner/repo`: only this project's tasks. */
  providerScopeId: string | null;
  providerScopeLabel: string | null;
  delivery: FactoryDelivery;
  mode: FactoryMode;
  appStartCmd: string | null;
  appUrl: string | null;
  /** The wizard step the user is on, 1–5; 6 once ▶ Play was pressed. */
  setupStep: number;
  loopState: FactoryLoopState;
  /** The sealed brain version new orders freeze. */
  brainVersion: number | null;
  createdAt: string;
  updatedAt: string;
}

/** What the wizard shows about a folder's repo (`factory::setup::RepoStatus`). */
export interface RepoStatus {
  exists: boolean;
  isRepo: boolean;
  hasCommits: boolean;
  remoteUrl: string | null;
  /** `owner/repo` when the remote is a forge Hangar knows. */
  forgePath: string | null;
  forge: 'github' | 'gitlab' | null;
  branch: string | null;
  hasMain: boolean;
  hasDev: boolean;
  /** What a task branch starts from: `dev` when there is one. */
  baseBranch: string | null;
}

export type NewFolder = { kind: 'existing'; path: string } | { kind: 'new'; parent: string; name: string };

export const FACTORY_STEPS = ['Proyecto', 'Equipo', 'Brain', 'Tareas', '▶ Play'] as const;

export const MODE_LABEL: Record<FactoryMode, string> = {
  supervised: 'Supervisado',
  delegated: 'Delegado',
  autonomous: 'Autónomo',
};

export const MODE_HINT: Record<FactoryMode, string> = {
  supervised: 'Para en el spec y en el diff: tú apruebas cada orden.',
  delegated: 'Solo para si el brain (Riesgo) marca riesgo.',
  autonomous: 'No para nunca: los riesgos quedan anotados y en la MR.',
};

/** Where the project's tasks come from, in a few words, for the header. */
export function providerShort(p: FactoryProject): string {
  if (p.providerKind === 'none') return 'local (Hangar)';
  const name = p.providerKind === 'clickup' ? 'ClickUp' : 'GitHub';
  return p.providerScopeLabel ? `${name} «${lastSegment(p.providerScopeLabel)}»` : name;
}

/** `Space / Folder / List` → `List`. */
export function lastSegment(path: string): string {
  const parts = path.split(' / ');
  return parts[parts.length - 1];
}

/** `/Users/me/Proyects/x` → `~/Proyects/x`, given the home folder. */
export function tildePath(path: string, home: string | null): string {
  return home && path.startsWith(home + '/') ? '~' + path.slice(home.length) : path;
}

// ---------------------------------------------------------------- brain

export const BRAIN_SECTIONS: [string, string][] = [
  ['prd', 'Producto'],
  ['arq', 'Arquitectura'],
  ['tec', 'Tecnologías'],
  ['cod', 'Código'],
  ['dat', 'Datos'],
  ['sec', 'Seguridad'],
  ['cal', 'Calidad'],
  ['git', 'Git y entrega'],
  ['dep', 'Despliegue'],
  ['rsg', 'Riesgo'],
  ['dec', 'Decisiones'],
];

export const sectionName = (key: string): string => BRAIN_SECTIONS.find(([k]) => k === key)?.[1] ?? key;

export interface BrainRule {
  id: string;
  section: string;
  level: 'must' | 'should';
  text: string;
  check: string | null;
}

/** What an import found (`factory::brain::Report`). */
export interface BrainReport {
  rules: BrainRule[];
  pending: string[];
  assigned: string[];
  unknown: string[];
  sections: string[];
  missing: string[];
  missingRequired: string[];
  checks: number;
  markdown: string;
}

export interface BrainVersion {
  version: number;
  path: string;
  sha: string;
  ruleCount: number;
  source: string;
  sealedAt: string;
}

export interface SealedBrain extends BrainVersion {
  rules: BrainRule[];
  markdown: string;
}

export interface Amendment {
  id: string;
  question: string;
  orderKey: string | null;
  provisional: string | null;
  status: 'open' | 'included' | 'dismissed';
  includedIn: number | null;
  createdAt: string;
}

export const levelLabel = (level: BrainRule['level']) => (level === 'must' ? 'DEBE' : 'DEBERÍA');

// ---------------------------------------------------------------- team

export interface FactoryAgent {
  id: string;
  role: 'spec' | 'critic' | 'builder' | 'tester' | 'reviewer_code' | 'reviewer_security' | 'reviewer_ui' | 'reviewer_ux' | 'verifier' | 'approver';
  group: string;
  name: string;
  short: string;
  duty: string;
  sections: string[];
  accountId: string | null;
  model: string;
  active: boolean;
  required: boolean;
  command: string | null;
  escalateModel: string | null;
  note: string | null;
  position: number;
}

export interface TeamCheck {
  problems: string[];
  warnings: string[];
}

export interface TeamView {
  agents: FactoryAgent[];
  check: TeamCheck;
}

export const GROUP_HINT: Record<string, string> = {
  Ingeniería: 'escriben la spec de cada orden',
  Fabricación: 'escribe el código',
  Testers: 'cada uno con su modelo y su comando',
  Revisores: 'en paralelo, cada uno con su sección del brain',
  Decisión: 'solo recibe órdenes con los tests en verde',
};

export const EXTRA_TESTERS = [
  { name: 'Tester de rendimiento', short: 'rendimiento', duty: 'tiempos de respuesta', command: 'locust --headless -t 1m' },
  { name: 'Tester de contrato', short: 'contrato', duty: 'la API cumple su OpenAPI', command: 'schemathesis run openapi.json' },
  { name: 'Tester de mutación', short: 'mutación', duty: '¿los tests detectan fallos?', command: 'mutmut run' },
];

/** `['cod','arq']` → `código · arquitectura`; empty means the rule cited. */
export function brainLabel(sections: string[]): string {
  return sections.length ? sections.map((s) => sectionName(s).toLowerCase()).join(' · ') : 'la regla citada';
}

// ---------------------------------------------------------------- orders

export type OrderStatus = 'todo' | 'running' | 'waiting_limit' | 'waiting_human' | 'stopped' | 'failed' | 'delivered' | 'merged';

export interface FactoryOrder {
  id: string;
  key: string;
  title: string;
  description: string;
  criteria: string[];
  deps: string[];
  position: number;
  status: OrderStatus;
  origin: 'provider' | 'trd' | 'manual';
  extId: string | null;
  extUrl: string | null;
  touchesUi: boolean;
  rules: string[];
  brainVersion: number | null;
  createdAt: string;
  blockedBy: string[];
}

export interface PlannedOrder {
  title: string;
  description: string;
  criteria: string[];
  deps: number[];
  trdPart: string;
  rules: string[];
  touchesUi: boolean;
}

export interface TrdPlan {
  orders: PlannedOrder[];
  coverage: { part: string; orders: number[]; rules: string[] }[];
  risks: string[];
  uncovered: string[];
}

/** The chip an order shows in the queue: colour class and words. */
export function orderChip(o: FactoryOrder, isNext: boolean): [string, string] {
  if (o.status === 'todo') return o.blockedBy.length ? ['red', 'bloqueada'] : isNext ? ['acc', '▶ siguiente'] : ['', 'por hacer'];
  const map: Record<OrderStatus, [string, string]> = {
    todo: ['', 'por hacer'],
    running: ['acc', 'en curso'],
    waiting_limit: ['amber', 'en espera'],
    waiting_human: ['amber', '✋ te espera'],
    stopped: ['', 'parada'],
    failed: ['red', 'fallida'],
    delivered: ['teal', 'MR abierta'],
    merged: ['grn', 'fusionada'],
  };
  return map[o.status];
}

/** Where the project's tasks live, in words, for hints. */
export function providerWhere(p: FactoryProject): string {
  if (p.providerKind === 'none') return 'el backlog local de Hangar';
  if (p.providerKind === 'clickup') return `la lista «${lastSegment(p.providerScopeLabel ?? 'ClickUp')}» de ClickUp`;
  return `las issues de ${p.providerScopeId ?? 'GitHub'}`;
}

// ---------------------------------------------------------------- the line

export type Station = 'ing' | 'fab' | 'test' | 'rev' | 'dec' | 'ent';

export const STATION_NAME: Record<Station, string> = {
  ing: 'Ingeniería',
  fab: 'Fabricación',
  test: 'Testers',
  rev: 'Revisión',
  dec: 'Decisión',
  ent: 'Entrega',
};

export interface GateResult {
  name: string;
  command: string;
  ok: boolean;
  tail: string;
}

export interface RunState {
  spec: Record<string, unknown> | null;
  feedback: string | null;
  findings: Record<string, unknown>[];
  warnings: string[];
  escalated: boolean;
  gateOk: boolean;
  gate: GateResult[];
  humanKind: 'spec_review' | 'diff' | 'approver' | 'credits' | 'merge' | null;
  human: string | null;
  summary: string | null;
}

export interface FactoryRun {
  id: string;
  projectId: string;
  orderId: string;
  status: 'running' | 'waiting_limit' | 'waiting_human' | 'stopped' | 'failed' | 'delivered' | 'merged';
  stage: string;
  round: number;
  team: FactoryAgent[];
  brainVersion: number;
  branch: string;
  worktree: string;
  base: string;
  resumeAt: string | null;
  mrUrl: string | null;
  costUsd: number;
  error: string | null;
  state: RunState;
  startedAt: string;
  finishedAt: string | null;
}

export interface StepRow {
  id: string;
  runId: string;
  agentId: string | null;
  label: string;
  model: string;
  station: Station;
  status: 'running' | 'done' | 'failed' | 'wait' | 'human' | 'skipped' | 'stopped';
  round: number;
  note: string | null;
  costUsd: number;
  startedAt: string;
  finishedAt: string | null;
}

export interface ActivityRow {
  id: number;
  stepId: string | null;
  station: Station;
  at: string;
  kind: 'read' | 'edit' | 'cmd' | 'brain' | 'warn' | 'gate' | 'text';
  text: string;
  ruleId: string | null;
}

export interface DecisionRow {
  stepId: string | null;
  station: Station;
  at: string;
  ruleId: string | null;
  text: string;
}

export interface DayRow {
  orderKey: string;
  title: string;
  status: string;
  resumeAt: string | null;
  steps: StepRow[];
}

export interface LiveData {
  run: FactoryRun | null;
  order: FactoryOrder | null;
  steps: StepRow[];
  activity: ActivityRow[];
  decisions: DecisionRow[];
  today: DayRow[];
  averages: Partial<Record<Station, number>>;
}

export interface Delivery {
  runId: string;
  orderKey: string;
  title: string;
  status: FactoryRun['status'];
  stage: string;
  mrUrl: string | null;
  rounds: number;
  roundRules: string[];
  rulesCited: number;
  costUsd: number;
  startedAt: string;
  finishedAt: string | null;
  error: string | null;
}

/** `2026-10-30T13:46:20Z` → `13:46`, local time. */
export function hhmm(iso: string): string {
  const d = new Date(iso);
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

/** Seconds → `0:13:40`. */
export function countdown(seconds: number): string {
  const s = Math.max(0, Math.floor(seconds));
  return `${Math.floor(s / 3600)}:${String(Math.floor((s % 3600) / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`;
}

// ---------------------------------------------------------------- preparation

/** One thing the line needs from the project (`factory::prepare::Check`). */
export interface PrepCheck {
  id: string;
  label: string;
  status: 'ok' | 'fixed' | 'todo' | 'warn' | 'blocked';
  detail: string;
  action: 'apply_testers' | 'team' | 'brain' | 'git_identity' | 'tasks' | null;
}

/** A tester whose command the brain says differently. */
export interface TesterSuggestion {
  agentId: string;
  name: string;
  current: string | null;
  currentActive: boolean;
  suggested: string | null;
  active: boolean;
}
