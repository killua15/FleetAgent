export type SessionMode = 'plan' | 'ask_edits' | 'auto_edit' | 'yolo';
export type SessionStatus = 'waiting' | 'running' | 'reviewing' | 'done';
export type BlockKind = 'you' | 'think' | 'edit' | 'cmd' | 'agent' | 'sum' | 'rev' | 'out';
export type Effort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

export interface Session {
  id: string;
  name: string;
  projectId: string;
  nodeId: string | null;
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
  createdAt: string;
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

export const MODE_LABEL: Record<SessionMode, string> = {
  plan: 'plan',
  ask_edits: 'ask edits',
  auto_edit: 'auto-edit',
  yolo: 'yolo',
};

export const EFFORT_OPTIONS: Effort[] = ['low', 'medium', 'high', 'xhigh', 'max'];

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
