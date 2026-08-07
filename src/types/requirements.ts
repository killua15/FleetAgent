/** What Hangar needs, derived from what each feature declares.
 *
 * The unit is a **capability** — something the user wants to do — not a tool.
 * A tool appears because some capability names it, and what it unlocks is that
 * capability's own words: the same sentence the feature shows when it refuses.
 */

export type CapabilityId =
  | 'run-agents'
  | 'git-work'
  | 'pull-requests'
  | 'skills-from-git-hub'
  | 'code-graph'
  | 'graph-to-agents';

export interface ToolStatus {
  name: string;
  installed: boolean;
  /** Where Hangar found it — the answer to "but it works in my terminal". */
  path: string | null;
  /** The command to run next, which is the prerequisite's when that is also
   * missing. */
  installHint: string | null;
  /** Named when this tool needs another first, so the UI can explain why it's
   * talking about `uv` when you asked about a knowledge graph. */
  blockedBy: string | null;
}

export interface CapabilityStatus {
  id: CapabilityId;
  label: string;
  enables: string;
  /** Hangar is useless without it. */
  essential: boolean;
  available: boolean;
  /** One sentence naming the missing step. */
  reason: string | null;
  installHint: string | null;
  tools: ToolStatus[];
}
