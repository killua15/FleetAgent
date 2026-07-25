export type PullRequestState = 'open' | 'closed' | 'merged';

export interface PullRequestInfo {
  id: string;
  sessionId: string;
  repoOwner: string;
  repoName: string;
  number: number;
  url: string;
  state: PullRequestState;
  lastCommentSeenAt: string | null;
  /** Error from the most recent failed check for this PR (permissions, SSO,
   * rate limit) — null once a check succeeds. Shown in the UI instead of a
   * silent "no new comments" so a persistently-failing check is visible. */
  lastCheckError: string | null;
  createdAt: string;
  /** The PR's real head branch — null until either Fleet created the PR
   * itself or "Send to agent" has backfilled it for a discovered one. */
  headBranch: string | null;
}

/** Read-only preview of what `create_pull_request` would produce for a
 * session — computed without any of its mutations (no branch creation, no
 * commit, no push). `body` is the exact markdown that would be sent to
 * `gh pr create`; `added`/`modified`/`removed` are the same file lists
 * already split out for a nicer preview than re-parsing the markdown. */
export interface PullRequestPreview {
  suggestedTitle: string;
  body: string;
  added: string[];
  modified: string[];
  removed: string[];
  linesAdded: number;
  linesRemoved: number;
}

/** One comment — a general PR comment/review, or an inline diff comment tied
 * to a file/line (`path`/`line` set) — see `models::PrComment` for why both
 * shapes are collapsed into this one. `id` is the real `pr_review_comments`
 * row once persisted, so "send"/"dismiss" always act on a real row. */
export interface PrComment {
  id: string;
  author: string;
  body: string;
  createdAt: string;
  path: string | null;
  line: number | null;
}

/** One open PR (with its still-pending review comments) inside a project's
 * group in the PR review inbox screen. */
export interface PrInboxEntry {
  pullRequest: PullRequestInfo;
  sessionId: string;
  sessionName: string;
  comments: PrComment[];
}

/** One project's group in the PR review inbox — every open PR that project
 * has, each with its pending (unresolved) comments. */
export interface ProjectPrInbox {
  projectId: string;
  projectName: string;
  entries: PrInboxEntry[];
}
