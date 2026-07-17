import type { Provider, Scope } from './account';

export interface AppSettings {
  reviewerEnabled: boolean;
  reviewerAccountId: string | null;
  notifyNeedsApproval: boolean;
  notifySessionWaiting: boolean;
  notifyAccountExhausted: boolean;
  notifySpendCap: boolean;
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
