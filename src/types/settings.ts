import type { Provider, Scope } from './account';

export interface AppSettings {
  reviewerEnabled: boolean;
  reviewerAccountId: string | null;
  notifyNeedsApproval: boolean;
  notifySessionWaiting: boolean;
  notifyAccountExhausted: boolean;
  notifySpendCap: boolean;
  notifyPrComments: boolean;
  notifyUpdateAvailable: boolean;
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

/**
 * What every agent should know before you type anything — the standing
 * preferences Hangar passes to the CLI at spawn time.
 *
 * `null` means "not set here". On a project that reads as **inherit the
 * global default**; on the global layer it means nothing is configured, and
 * Hangar passes no flag at all rather than an empty one.
 *
 * The two languages are separate on purpose: replying in Spanish while
 * commenting an English codebase in English is the common case, not an edge
 * one. Values are the language's own name (see `lib/languages.ts`), since
 * that string goes straight into the instruction the agent reads.
 */
export interface AgentPreferences {
  conversationLanguage: string | null;
  commentLanguage: string | null;
}

/**
 * One skill in your library — a folder with a `SKILL.md`, in the open Agent
 * Skills format that Claude Code, Codex, Gemini CLI and others all read.
 *
 * Lives on disk under Hangar's app data rather than in the database, because
 * the standard says a skill is a *folder*: it can also carry `scripts/`,
 * `references/` and `assets/`. Only which projects turn it on is stored in
 * SQLite.
 */
export interface Skill {
  /** The folder name, which is also the slash command (`/<name>`). */
  name: string;
  /** Load-bearing, not a label: it is all an agent sees until it decides to
   * open the skill, so it is what determines whether the skill ever fires. */
  description: string;
  body: string;
  /** Whether the folder holds more than `SKILL.md` — scripts, references. */
  hasExtraFiles: boolean;
}

/** A GitHub repository you can install skills from. Holds no credential —
 * `gh` brings its own login, which is why a private team repo works. */
export interface SkillSource {
  id: string;
  /** Always `owner/repo`, normalised from whatever was typed or pasted. */
  repo: string;
  label: string;
}

/** A skill seen in a repository, not installed yet. */
export interface RemoteSkill {
  name: string;
  description: string;
  installed: boolean;
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

/** What Hangar does when a session's account runs out of quota. Stored per
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
