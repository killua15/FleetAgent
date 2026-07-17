import type { SessionBlock } from '../types/session';

export interface Turn {
  /** Key for React + collapsed-state tracking — the id of the turn's leading
   * 'you' block (or the first block's id, for any stray blocks that arrive
   * before a first prompt, which shouldn't normally happen but stay visible
   * in their own turn rather than being dropped). */
  id: string;
  blocks: SessionBlock[];
}

/** Groups a session's blocks into turns — a new turn starts at each 'you'
 * block (the user's prompt) and includes everything up to the next one. */
export function groupBlocksIntoTurns(blocks: SessionBlock[]): Turn[] {
  const turns: Turn[] = [];
  let current: Turn | null = null;

  for (const block of blocks) {
    if (block.kind === 'you' || !current) {
      current = { id: block.id, blocks: [] };
      turns.push(current);
    }
    current.blocks.push(block);
  }
  return turns;
}
