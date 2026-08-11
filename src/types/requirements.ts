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
  | 'merge-requests'
  | 'skills-from-git-hub'
  | 'code-graph'
  | 'graph-to-agents'
  | 'codex-accounts'
  | 'gemini-accounts'
  | 'grok-accounts';

export interface ToolStatus {
  name: string;
  installed: boolean;
  /** Where Hangar found it — the answer to "but it works in my terminal". */
  path: string | null;
  /** The command to run next, which is the prerequisite's when that is also
   * missing. Picked for this machine — Homebrew's route only when brew exists. */
  installHint: string | null;
  /** The other route. Homebrew is not on every Mac, and a command you can't
   * run is the dead end this whole thing exists to stop. */
  alternative: string | null;
  /** The step that isn't a command — logging in, or adding an account. A
   * provider's CLI signs nobody in, so a guide that stops at the install
   * leaves you one step short and thinking you're done. */
  then: string | null;
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
