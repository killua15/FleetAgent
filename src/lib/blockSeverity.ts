import type { SessionBlock } from '../types/session';

export type Severity = 'error' | 'notable' | 'routine';

/**
 * "Color = severity, not decoration" — a normal block stays dim/neutral;
 * color is reserved for genuine exceptions. `out` blocks Hangar's own backend
 * already prefixes with `"error: "` (see turn_runner.rs::parse_line) become
 * `error`; a turn's own final response (`agent`/`sum`) stays visually
 * prominent as `notable`; everything else (thinking, commands, ordinary
 * output, `you`/`edit`/`rev` which already have their own identity) is
 * `routine` and shouldn't compete for attention.
 */
export function severityOfBlock(block: Pick<SessionBlock, 'kind' | 'text'>): Severity {
  if (block.kind === 'out' && block.text.startsWith('error: ')) return 'error';
  if (block.kind === 'agent' || block.kind === 'sum') return 'notable';
  return 'routine';
}

/** Severity of the most recent block in a list — used to decide whether a
 * session's card/row should visually flag that it needs attention right now. */
export function severityOfLastBlock(blocks: Pick<SessionBlock, 'kind' | 'text'>[]): Severity {
  const last = blocks[blocks.length - 1];
  return last ? severityOfBlock(last) : 'routine';
}
