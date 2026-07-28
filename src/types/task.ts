/** One task, whatever board it came from.
 *
 * Deliberately small — the same shape for a GitHub issue and a ClickUp task.
 * Sprints, points, custom fields and relations stay out: the moment one tool's
 * concept enters the model, every other source has to fake it. */
export interface BoardTask {
  /** The tool's own id, stable between passes. */
  externalId: string;
  /** The short human key that goes in a branch: `128`, `DEV-42`, `CU-86abc`. */
  key: string;
  title: string;
  description: string;
  url: string;
  status: string | null;
  branchHint: string | null;
  sourceId: string;
  sourceKind: string;
  /** Where it lives — a ClickUp list, a GitHub repo. "Tests" means something
   * different under *Viewer* than under *Orquestrador*. */
  container: string | null;
  /** Already templated and made safe for `git branch` by the backend — the
   * panel prefills the row with it, and you can still edit it. */
  suggestedBranch: string;
}

/** What a fetch across a project's sources came back with.
 *
 * `errors` rather than a failure because one broken source must not hide the
 * others: an expired ClickUp token shouldn't take your GitHub issues with it. */
export interface TaskFetch {
  tasks: BoardTask[];
  errors: string[];
}

/** Something a project can be pointed at inside a source — a ClickUp list.
 * GitHub returns none: its scope is the project's own remote. */
export interface SourceScope {
  id: string;
  /** `Space / Folder / List` — "Tasks" is a list name in four places. */
  path: string;
}

/** A configured source. No token here, ever — it lives in the OS keychain. */
export interface TaskSource {
  id: string;
  kind: TaskSourceKind;
  label: string;
  hasCredential: boolean;
  config: Record<string, unknown>;
}

export type TaskSourceKind = 'github' | 'clickup';

/** How one project is wired to one source. */
export interface TaskSourceLink {
  sourceId: string;
  kind: TaskSourceKind;
  label: string;
  /** Opaque: a ClickUp list id, an `owner/repo`. Empty means the source's
   * default — for GitHub, the project's own remote; for ClickUp, everything
   * assigned to you across the workspace. */
  scopeId: string | null;
  branchTemplate: string | null;
}

export const TASK_SOURCE_META: Record<TaskSourceKind, { name: string; needsToken: boolean; hint: string }> = {
  github: {
    name: 'GitHub Issues',
    needsToken: false,
    hint: 'Uses the account `gh` is already logged into — nothing to configure.',
  },
  clickup: {
    name: 'ClickUp',
    needsToken: true,
    hint: 'Personal API token from ClickUp → Settings → Apps. One token covers every workspace you belong to.',
  },
};
