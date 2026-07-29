import { describe, expect, it } from 'vitest';
import { compactTokens, money } from './formatUsage';

describe('compactTokens', () => {
  it('switches units where the eye expects it', () => {
    expect(compactTokens(999)).toBe('999');
    expect(compactTokens(1_000)).toBe('1K');
    expect(compactTokens(28_220)).toBe('28K');
    expect(compactTokens(511_994)).toBe('512K');
    expect(compactTokens(1_200_000)).toBe('1.2M');
  });

  it('shows a real zero', () => {
    // A session with no turns is common — a blank cell would read as a
    // rendering failure rather than as "nothing yet".
    expect(compactTokens(0)).toBe('0');
  });
});

describe('money', () => {
  it('keeps three decimals below a dollar', () => {
    // The measured cost of one turn is $0.003. Two decimals would print
    // `$0.00` on every row and make the table look broken.
    expect(money(0.003)).toBe('$0.003');
    expect(money(0.154)).toBe('$0.154');
    expect(money(0)).toBe('$0.000');
  });

  it('switches to cents at a dollar, where cents are what people read', () => {
    expect(money(1)).toBe('$1.00');
    expect(money(22.4)).toBe('$22.40');
  });
});
