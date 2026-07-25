import type { Provider, Scope } from './account';

export interface AppSettings {
  reviewerEnabled: boolean;
  reviewerAccountId: string | null;
  notifyNeedsApproval: boolean;
  notifySessionWaiting: boolean;
  notifyAccountExhausted: boolean;
  notifySpendCap: boolean;
  notifyPrComments: boolean;
  /** Opt-in gate for the tree-canvas feature (hierarchical agent teams) —
   * off by default, hides the "Hierarchy trees" icon in `AppTopBar` until
   * turned on. */
  agentTreesEnabled: boolean;
  /** Off by default: PR review comments normally drop anything written by
   * the logged-in `gh` user themselves (the feature surfaces *other*
   * reviewers' feedback) — turning this on keeps self-authored comments
   * too, for testing the review flow solo. */
  includeOwnPrComments: boolean;
}

/** One row of the Rotation checklist — an account plus whether the given
 * project may use it. */
export interface ProjectAccountRotation {
  accountId: string;
  accountName: string;
  provider: Provider;
  scope: Scope;
  allowed: boolean;
}

/** What Fleet does when a session's account runs out of quota. Stored per
 * (project, provider) in `rotation_policies`; see `src-tauri/src/rotation.rs`
 * for the ranking rules. */
export type RotationPolicy = 'manual' | 'auto' | 'round_robin';

export const ROTATION_POLICY_OPTIONS: Array<{ value: RotationPolicy; label: string; hint: string }> = [
  { value: 'manual', label: 'Manual', hint: 'Never switch accounts. Sessions stop when their account runs out.' },
  { value: 'auto', label: 'Least loaded', hint: 'Switch to the allowed account with the least recorded usage this week.' },
  { value: 'round_robin', label: 'Round robin', hint: 'Switch to the allowed account that went longest without being used.' },
];

export interface MemoryGroup {
  id: string;
  name: string;
  createdAt: string;
}

export interface MemoryGroupSummary {
  id: string;
  name: string;
  projectCount: number;
  fileCount: number;
}

export interface GroupMembership {
  projectId: string;
  projectName: string;
  member: boolean;
}

export interface MemoryFile {
  id: string;
  projectId: string;
  projectName: string;
  filename: string;
  contentMd: string;
  updatedAt: string;
}

export interface MemoryFileSummary {
  id: string;
  projectId: string;
  projectName: string;
  filename: string;
  updatedAt: string;
}

/** A real `.md` file already on disk in a project, not yet imported into
 * memory_files. */
export interface DiscoveredMemoryFile {
  projectId: string;
  projectName: string;
  relativePath: string;
}
