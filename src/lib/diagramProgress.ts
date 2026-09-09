/** How far along a diagram turn is, derived from the blocks it streams.
 *
 * **There is no percentage here on purpose.** A turn has no known total — the
 * agent decides how many validation rounds it needs, and archify's own flow is
 * "repair until the error count stops improving". Any bar sitting at 40% would
 * be decoration, and a progress bar that lies is worse than none: it turns
 * "this is taking a while" into "this is stuck".
 *
 * What is real is the stage, and each one is only claimed when the evidence for
 * it comes past — a spec being written, `archify.mjs validate` being run,
 * `deliver` being run. So the display is always behind the truth rather than
 * ahead of it.
 */
export type DiagramStage = 'starting' | 'drafting' | 'validating' | 'delivering' | 'done' | 'failed';

export interface DiagramProgress {
  stage: DiagramStage;
  /** How many times the agent has run `validate`. Genuinely informative:
   * archify's checks are strict, and the diagram in this repo took four
   * rounds to pass. Seeing "attempt 3" is the difference between "it is
   * working" and "it is stuck". */
  validationRounds: number;
  /** The last thing it actually did, short enough for one line. */
  lastAction: string | null;
}

export const IDLE: DiagramProgress = { stage: 'starting', validationRounds: 0, lastAction: null };

/** The order stages are shown in. `done` and `failed` are terminal and are not
 * steps of their own — they light or extinguish the last one. */
export const STAGES: DiagramStage[] = ['starting', 'drafting', 'validating', 'delivering'];

/** Folds one streamed block into the progress so far.
 *
 * Never goes backwards except `delivering → validating`, which is not a
 * regression but what actually happens: a failed delivery sends the agent back
 * to fix the spec, and saying so beats freezing on a stage it has left.
 */
export function advance(current: DiagramProgress, block: { kind: string; text: string }): DiagramProgress {
  const text = block.text;

  if (block.kind === 'cmd' && /archify/i.test(text)) {
    if (/\bvalidate\b/.test(text)) {
      return { stage: 'validating', validationRounds: current.validationRounds + 1, lastAction: 'comprobando el diagrama' };
    }
    if (/\bdeliver\b/.test(text)) {
      return { ...current, stage: 'delivering', lastAction: 'compilando la página' };
    }
  }

  // The spec is the artifact the page is compiled from, so writing one is the
  // first thing that counts as real progress. A `.json` under any name: the
  // agent picks the file name, not us.
  if (block.kind === 'edit' && /\.json\b/i.test(text)) {
    const stage = current.stage === 'starting' ? 'drafting' : current.stage;
    return { ...current, stage, lastAction: 'escribiendo la especificación' };
  }

  // Anything else only moves the "what is it doing" line, never the stage —
  // reading files and thinking are not milestones.
  if (block.kind === 'cmd') return { ...current, lastAction: firstWords(text.replace(/^\$\s*/, ''), 60) };
  if (block.kind === 'edit' || block.kind === 'agent' || block.kind === 'think') {
    return { ...current, lastAction: firstWords(text, 60) };
  }
  return current;
}

/** Whether the run reached a page, judged by what it did rather than by the
 * session merely stopping — a turn that dies halfway also stops. */
export function finish(current: DiagramProgress): DiagramProgress {
  return current.stage === 'delivering'
    ? { ...current, stage: 'done', lastAction: null }
    : { ...current, stage: 'failed', lastAction: current.lastAction };
}

/** One line, trimmed, no newlines — this goes in a single row. */
function firstWords(text: string, max: number): string {
  const flat = text.replace(/\s+/g, ' ').trim();
  return flat.length <= max ? flat : `${flat.slice(0, max - 1)}…`;
}

/** What to call each stage in the UI. */
export const STAGE_LABEL: Record<DiagramStage, string> = {
  starting: 'arrancando',
  drafting: 'especificando',
  validating: 'comprobando',
  delivering: 'compilando',
  done: 'listo',
  failed: 'sin terminar',
};
