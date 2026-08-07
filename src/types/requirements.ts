/** What Hangar needs on the machine, and what is missing. */
export interface Requirement {
  id: string;
  label: string;
  /** `required` — nothing works without it. `optional` — one named feature is
   * unavailable and everything else is fine. */
  need: 'required' | 'optional';
  /** What having it buys, in the user's terms. */
  unlocks: string;
  installed: boolean;
  /** Absolute path when found — "it works in my terminal" and "Hangar can find
   * it" are different claims for an app launched from Finder. */
  path: string | null;
  /** The command to run *next*, which is the prerequisite's when this tool's
   * own prerequisite is missing. */
  installHint: string | null;
  /** Set when the hint above belongs to a prerequisite, so the UI can say why
   * it's talking about `uv` when you asked about a knowledge graph. */
  blockedBy: string | null;
}
