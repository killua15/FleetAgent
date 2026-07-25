import type { FileStatus } from './session';

/** One local or remote-tracking branch — matches `BranchInfo` in `models.rs`.
 * `ahead`/`behind` are `0` when there's no `upstream`. */
export interface BranchInfo {
  name: string;
  isRemote: boolean;
  isCurrent: boolean;
  upstream: string | null;
  ahead: number;
  behind: number;
  lastCommitHash: string;
  lastCommitSubject: string;
  lastCommitDate: string;
}

/** One commit row — matches `CommitInfo` in `models.rs`. `parents` (full
 * hashes) is what `commitGraph.ts`'s lane layout consumes to draw the DAG. */
export interface CommitInfo {
  hash: string;
  shortHash: string;
  parents: string[];
  author: string;
  date: string;
  subject: string;
  refs: string[];
}

/** One page of `list_commits` — matches `CommitLogPage` in `models.rs`. */
export interface CommitLogPage {
  commits: CommitInfo[];
  hasMore: boolean;
}

/** One file changed by a commit — matches `CommitFileChange` in `models.rs`;
 * reuses the same `FileStatus` the session file tree already uses. */
export interface CommitFileChange {
  path: string;
  status: FileStatus;
}

/** A single file's before/after content for one commit — matches
 * `CommitFileDiff` in `models.rs`; feed straight into `diffLines()`. */
export interface CommitFileDiff {
  oldText: string;
  newText: string;
}
