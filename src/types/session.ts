export type SessionMode = 'plan' | 'ask_edits' | 'auto_edit' | 'yolo';
export type SessionStatus = 'waiting' | 'running' | 'reviewing' | 'done';
export type BlockKind = 'you' | 'think' | 'edit' | 'cmd' | 'agent' | 'sum' | 'rev' | 'out';
export type Effort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export type WorktreeKind = 'linked' | 'copy';

export interface Session {
  id: string;
  name: string;
  projectId: string;
  nodeId: string | null;
  parentSessionId: string | null;
  worktreePath: string | null;
  worktreeBranch: string | null;
  worktreeKind: WorktreeKind | null;
  accountId: string;
  model: string;
  effort: Effort;
  mode: SessionMode;
  status: SessionStatus;
  pid: number | null;
  startedAt: string;
  endedAt: string | null;
}

export interface SessionView {
  id: string;
  name: string;
  projectId: string;
  projectName: string;
  parentSessionId: string | null;
  worktreeBranch: string | null;
  worktreeKind: WorktreeKind | null;
  accountId: string;
  accountName: string;
  provider: 'claude' | 'codex' | 'gemini';
  scope: 'work' | 'personal';
  model: string;
  effort: Effort;
  mode: SessionMode;
  status: SessionStatus;
  startedAt: string;
}

export interface SessionBlock {
  id: string;
  sessionId: string;
  kind: BlockKind;
  text: string;
  createdAt: string;
}

export interface Project {
  id: string;
  name: string;
  path: string;
  defaultSetupCommand: string | null;
  createdAt: string;
}

export interface NewSubsession {
  parentSessionId: string;
  name: string;
  accountId: string;
  model: string;
  effort: Effort;
  mode: SessionMode;
  setupCommand?: string;
}

export interface ModelOption {
  id: string;
  label: string;
}

export interface ResumableConversation {
  providerSessionId: string;
  preview: string;
  lastActiveAt: string;
}

export interface FileEdit {
  id: string;
  sessionId: string;
  blockId: string | null;
  filePath: string;
  toolName: string;
  oldText: string | null;
  newText: string | null;
  createdAt: string;
}

export interface TurnUsage {
  id: string;
  sessionId: string;
  blockId: string | null;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number | null;
  cacheCreationTokens: number | null;
  costUsd: number | null;
  createdAt: string;
}

/** Fleet's own recorded spend for an account over a trailing window — not a
 * live pull of the provider's real quota. */
export interface AccountUsageSummary {
  inputTokens: number;
  outputTokens: number;
  costUsd: number;
}

/** Best-effort real usage scraped from Claude Code's interactive `/usage`
 * command — each field is `null` when it couldn't be parsed from the
 * rendered TUI text, in which case the caller should fall back to its own
 * token-based estimate rather than showing nothing. */
export interface RealUsageStats {
  sessionPct: number | null;
  weeklyPct: number | null;
  contextPct: number | null;
  rawText: string;
  fetchedAt: string;
}

/** Mirrors VS Code's file-explorer convention: untracked/added → green,
 * modified → amber, deleted → red. `touched` is the best-effort fallback for
 * a copy-kind subsession with no git history to ask. */
export type FileStatus = 'added' | 'modified' | 'deleted' | 'untracked' | 'touched';

export interface FileTreeNode {
  path: string;
  name: string;
  isDir: boolean;
  status: FileStatus | null;
  children: FileTreeNode[];
}

export const MODE_LABEL: Record<SessionMode, string> = {
  plan: 'plan',
  ask_edits: 'ask edits',
  auto_edit: 'auto-edit',
  yolo: 'yolo',
};

export const EFFORT_OPTIONS: Effort[] = ['low', 'medium', 'high', 'xhigh', 'max'];
export const MODE_OPTIONS: SessionMode[] = ['plan', 'ask_edits', 'auto_edit', 'yolo'];

export const BLOCK_TAG: Record<BlockKind, string> = {
  you: 'YOU',
  think: 'THINKING',
  edit: 'EDIT',
  cmd: 'RUN',
  agent: 'AGENT',
  sum: 'CHILD SUMMARY',
  rev: 'REVIEW',
  out: 'OUT',
};
