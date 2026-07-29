import { describe, expect, it } from 'vitest';
import { autoGridColumns } from './gridLayout';

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
