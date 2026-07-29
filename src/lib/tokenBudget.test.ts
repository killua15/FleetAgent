import { describe, expect, it } from 'vitest';
import {
  contextSeverity,
  contextSeverityColor,
  contextWindowFor,
  formatTokenCount,
  formatUsd,
  sessionTokenBudgetFor,
  weeklyTokenBudgetFor,
} from './tokenBudget';

describe('contextSeverity', () => {
  it('changes at the boundary, not past it', () => {
    // Exactly 70 is already worth watching and exactly 90 is already urgent.
    // Off-by-one here means a bar that stays green while the session is about
    // to be compacted out from under the user.
    expect(contextSeverity(69.9)).toBe('ok');
    expect(contextSeverity(70)).toBe('warn');
    expect(contextSeverity(89.9)).toBe('warn');
    expect(contextSeverity(90)).toBe('danger');
  });

  it('does not wrap around past 100', () => {
    // A percentage can overshoot when the estimate is off; it must stay red
    // rather than falling back to green.
    expect(contextSeverity(140)).toBe('danger');
  });

  it('gives each level its own colour', () => {
    const colors = (['ok', 'warn', 'danger'] as const).map(contextSeverityColor);
    expect(new Set(colors).size).toBe(3);
  });
});

describe('provider budgets', () => {
  it('knows each provider apart, and falls back rather than returning zero', () => {
    expect(contextWindowFor('claude')).toBe(200_000);
    expect(contextWindowFor('codex')).toBe(128_000);
    // An unknown provider — a new one added to the backend before this table —
    // must get a usable window. Zero would make every percentage infinite.
    expect(contextWindowFor('something-new')).toBeGreaterThan(0);
  });

  it('scales the session and week budgets off the same window', () => {
    // They are estimates, and the only property worth pinning is that they
    // stay ordered and proportional: a weekly budget under a session budget
    // would paint every account as over quota.
    for (const provider of ['claude', 'codex', 'gemini', 'unknown']) {
      expect(sessionTokenBudgetFor(provider)).toBeGreaterThan(contextWindowFor(provider));
      expect(weeklyTokenBudgetFor(provider)).toBeGreaterThan(sessionTokenBudgetFor(provider));
    }
  });
});

describe('formatTokenCount', () => {
  it('switches units at the right size and keeps one decimal', () => {
    expect(formatTokenCount(999)).toBe('999');
    expect(formatTokenCount(1000)).toBe('1.0k');
    expect(formatTokenCount(28_220)).toBe('28.2k');
    expect(formatTokenCount(1_000_000)).toBe('1.0m');
    expect(formatTokenCount(511_994)).toBe('512.0k');
  });

  it('shows zero as zero rather than as an empty string', () => {
    expect(formatTokenCount(0)).toBe('0');
  });
});

describe('formatUsd', () => {
  it('always shows cents', () => {
    // `$3` and `$3.00` read differently in a column of figures.
    expect(formatUsd(3)).toBe('$3.00');
    expect(formatUsd(0.1544)).toBe('$0.15');
    expect(formatUsd(0)).toBe('$0.00');
  });
});
