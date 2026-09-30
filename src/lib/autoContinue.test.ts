import { describe, expect, it } from 'vitest';
import { formatResumeAt } from './autoContinue';

/**
 * The only thing the panel says about a terminal waiting out its limit is when
 * it will carry on — a weekly reset days away must not read as "15:03".
 */
describe('formatResumeAt', () => {
  const now = new Date(2026, 8, 30, 10, 0); // Wed 30 Sep 2026, 10:00 local

  it('says today for a reset later today', () => {
    expect(formatResumeAt(new Date(2026, 8, 30, 15, 3).toISOString(), now)).toBe('today 15:03');
  });

  it('says tomorrow for the next day', () => {
    expect(formatResumeAt(new Date(2026, 9, 1, 9, 2).toISOString(), now)).toBe('tomorrow 09:02');
  });

  it('names the weekday within the week — the weekly window', () => {
    expect(formatResumeAt(new Date(2026, 9, 3, 9, 2).toISOString(), now)).toBe('Sat 09:02');
  });

  it('gives the date beyond a week', () => {
    expect(formatResumeAt(new Date(2026, 9, 12, 9, 2).toISOString(), now)).toBe('Oct 12 09:02');
  });
});
