import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { revealChildren } from './motion';

/** Just enough of an element for `revealChildren`: a flat block at `top`. */
function block(top: number) {
  const el = {
    parentElement: null,
    contains: (other: unknown) => other === el,
    closest: () => null,
    getBoundingClientRect: () => ({ top, bottom: top + 20, width: 200, height: 20 }),
    animate: vi.fn(),
  };
  return el;
}

function rootOf(blocks: ReturnType<typeof block>[]) {
  return { animate: () => {}, querySelectorAll: () => blocks } as unknown as HTMLElement;
}

describe('revealChildren', () => {
  beforeEach(() => {
    vi.stubGlobal('window', { innerHeight: 1000, matchMedia: () => ({ matches: false }) });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('never replays a block that was already on screen past the cap', () => {
    // 40 visible rows: more than the 28 a pass animates.
    const blocks = Array.from({ length: 40 }, (_, i) => block(i * 24));
    const seen = new WeakSet<Element>();

    revealChildren(rootOf(blocks), seen);
    const animated = blocks.filter((b) => b.animate.mock.calls.length > 0);
    expect(animated).toHaveLength(28);

    // The later passes (180 / 450 ms) must not pull rows 29–40 back to
    // opacity 0 — they were showing all along.
    revealChildren(rootOf(blocks), seen);
    revealChildren(rootOf(blocks), seen);
    expect(blocks.every((b) => b.animate.mock.calls.length <= 1)).toBe(true);
    expect(blocks.filter((b) => b.animate.mock.calls.length > 0)).toHaveLength(28);
  });

  it('still animates what arrives late, below the fold or after loading', () => {
    const first = Array.from({ length: 3 }, (_, i) => block(i * 24));
    const seen = new WeakSet<Element>();
    revealChildren(rootOf(first), seen);

    const late = block(200);
    revealChildren(rootOf([...first, late]), seen);
    expect(late.animate).toHaveBeenCalledTimes(1);
    expect(first.every((b) => b.animate.mock.calls.length === 1)).toBe(true);
  });

  it('leaves off-screen blocks for a later pass', () => {
    const below = block(5000);
    const seen = new WeakSet<Element>();
    revealChildren(rootOf([below]), seen);
    expect(below.animate).not.toHaveBeenCalled();
    expect(seen.has(below as unknown as Element)).toBe(false);
  });
});
