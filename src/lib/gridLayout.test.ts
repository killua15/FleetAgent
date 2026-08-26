import { describe, expect, it } from 'vitest';
import { autoGridColumns, shouldWarnAboutPanels } from './gridLayout';

/** Shared by the agent grid and the terminal grid, which is the point: two
 * surfaces laid out by different rules read as two unrelated screens. */
describe('autoGridColumns', () => {
  it('keeps one or two panels on a single row', () => {
    expect(autoGridColumns(1)).toBe(1);
    expect(autoGridColumns(2)).toBe(2);
  });

  it('splits three or more into exactly two rows, as evenly as it can', () => {
    // Never a third row, and never one ever-widening row: past six panels a
    // terminal is unreadable either way, and the sidebar is the answer.
    expect(autoGridColumns(3)).toBe(2); // 2 + 1
    expect(autoGridColumns(4)).toBe(2); // 2 + 2
    expect(autoGridColumns(5)).toBe(3); // 3 + 2
    expect(autoGridColumns(6)).toBe(3); // 3 + 3
    expect(autoGridColumns(8)).toBe(4);
  });

  it('never returns zero', () => {
    // The result divides a width. Zero columns is a division by zero in CSS
    // grid terms and collapses the whole screen.
    expect(autoGridColumns(0)).toBe(1);
    expect(autoGridColumns(-3)).toBe(1);
  });
});

/** The banner is advice, not a cap — which is why the number is the user's to
 * pick. The whole decision is two numbers, so it is worth pinning down. */
describe('shouldWarnAboutPanels', () => {
  it('waits until the panel count reaches the threshold', () => {
    expect(shouldWarnAboutPanels(3, 4)).toBe(false);
    expect(shouldWarnAboutPanels(4, 4)).toBe(true);
    expect(shouldWarnAboutPanels(9, 4)).toBe(true);
  });

  it('reads zero as never, whatever is on screen', () => {
    expect(shouldWarnAboutPanels(0, 0)).toBe(false);
    expect(shouldWarnAboutPanels(20, 0)).toBe(false);
  });

  it('treats a nonsense threshold as never, not as always', () => {
    // Only reachable from a corrupted setting; "always" would be the worse
    // of the two readings, since the banner cannot be dismissed.
    expect(shouldWarnAboutPanels(1, -3)).toBe(false);
  });
});
