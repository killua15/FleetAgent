import type { PullRequestInfo } from './pullRequest';

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
  defaultBranch: string | null;
  currentBranch: string | null;
  ahead: number;
  behind: number;
  dirty: boolean;
  worktrees: WorktreeGitStatus[];
}
