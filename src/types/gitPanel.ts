import type { Forge, PullRequestInfo } from './pullRequest';

/** One active worktree (linked `git worktree` or copied folder) branched off
 * a project, with its live git status — matches `WorktreeGitStatus` in
 * `models.rs`. */
export interface WorktreeGitStatus {
  sessionId: string;
  sessionName: string;
  branch: string | null;
  kind: string;
  path: string;
  ahead: number;
  behind: number;
  dirty: boolean;
  linkedPr: PullRequestInfo | null;
}

/** One project's repo status in the git panel — its own root checkout plus
 * every active worktree branched off it. Matches `ProjectGitOverview` in
 * `models.rs`. */
export interface ProjectGitOverview {
  projectId: string;
  projectName: string;
  projectPath: string;
  repoOwner: string | null;
  repoName: string | null;
  /** Which hosting product `origin` points at, and which instance — the same
   * pair `PullRequestInfo` carries, so both screens filter by one value. Null
   * for a project that isn't a git repo, or whose host can't be placed. */
  forge: Forge | null;
  host: string | null;
  defaultBranch: string | null;
  currentBranch: string | null;
  ahead: number;
  behind: number;
  dirty: boolean;
  worktrees: WorktreeGitStatus[];
}

/** One file in a worktree's change list. */
export interface ChangedFile {
  path: string;
  status: 'added' | 'modified' | 'deleted' | 'untracked' | 'touched';
  added: number;
  removed: number;
  /** Shown, not acted on — this view is read-only. */
  staged: boolean;
}

/** What a worktree holds, split the way you act on it.
 *
 * Two lists rather than one: `uncommitted` answers "what is the agent doing to
 * my repo right now", `committed` answers "what did this task produce". The
 * same file can be in both, with different content. */
export interface WorktreeChanges {
  /** The branch's upstream when it has one, else the project's default branch
   * — the same base the card's ahead/behind uses. */
  base: string;
  uncommitted: ChangedFile[];
  committed: ChangedFile[];
}
