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
  /** The wizard step the user is on, 1–5. */
  setupStep: number;
  loopState: FactoryLoopState;
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

/** The phase each later step lands in, shown on the locked steps. */
export const STEP_PHASE: Record<number, string> = { 2: 'F2', 3: 'F1', 4: 'F3', 5: 'F5' };

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
