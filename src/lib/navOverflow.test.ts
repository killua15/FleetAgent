import { describe, expect, it } from 'vitest';
import { fitCount } from './navOverflow';

const kinds = ['item', 'item', 'rule', 'item', 'item'] as const;
const widths = [100, 100, 11, 100, 100];

describe('fitCount', () => {
  it('keeps everything when it all fits', () => {
    // 8 + 4 * (100 + 2) + (11 + 2) = 429
    expect(fitCount(widths, [...kinds], 429, 8, 2, 40)).toBe(5);
  });

  it('drops from the end and leaves room for More', () => {
    // Two items (8 + 204) plus More (40 + 2) = 254 fits in 260; the rule after them is dropped.
    expect(fitCount(widths, [...kinds], 260, 8, 2, 40)).toBe(2);
  });

  it('never ends on a separator', () => {
    // Room for two items and the rule (8 + 204 + 13 = 225) plus More (42) = 267.
    expect(fitCount(widths, [...kinds], 300, 8, 2, 40)).toBe(2);
  });

  it('can collapse everything into More', () => {
    expect(fitCount(widths, [...kinds], 60, 8, 2, 40)).toBe(0);
  });
});
