import type { Forge, PullRequestInfo } from './pullRequest';

/** One worktree of a project, with its live git status — a session's (linked
 * `git worktree` or copied folder), or an orphan git lists but no session
 * owns. Matches `WorktreeGitStatus` in `models.rs`. */
export interface WorktreeGitStatus {
  /** `null` for an orphan. */
  sessionId: string | null;
  /** The session's name, or the directory's for an orphan. */
  sessionName: string;
  /** The session's account; `null` for an orphan. */
  accountId: string | null;
  branch: string | null;
  kind: string;
  path: string;
  ahead: number;
  behind: number;
  dirty: boolean;
  linkedPr: PullRequestInfo | null;
  /** The folder is gone — no git status; prune clears it. */
  missing: boolean;
  /** Outside `<project>/.hangar/worktrees`, so most likely another tool's. */
  external: boolean;
}

/** What `prune_worktrees` cleaned up. Matches `WorktreePruneResult` in `models.rs`. */
export interface WorktreePruneResult {
  prunedRegistrations: number;
  clearedSessions: number;
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

/** A live worktree as the Tasks panel needs it — matches `TaskWorktree` in
 * `git_panel.rs`. No git status: it's only for telling which tasks have one. */
export interface TaskWorktree {
  projectId: string;
  projectName: string;
  /** `owner/name` of the project's origin. */
  repo: string | null;
  path: string;
  branch: string | null;
  /** The folder's own name, e.g. `cu-86akmv`. */
  folder: string;
  sessionId: string | null;
}
