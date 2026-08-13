import { describe, expect, it } from 'vitest';
import { summarizePlan } from './plan';

describe('summarizePlan', () => {
  /** No subscription row is the normal state of a new account, not a failure —
   * and the account section has to say something in that space. */
  it('calls an account with no subscription Free', () => {
    expect(summarizePlan(null)).toEqual({ label: 'Free', paid: false, note: null, needsAttention: false });
  });

  it('names the plan and marks the paid ones', () => {
    const pro = summarizePlan({ plan: 'pro', status: 'active', currentPeriodEnd: null });
    expect(pro.label).toBe('Pro');
    expect(pro.paid).toBe(true);

    expect(summarizePlan({ plan: 'free', status: 'active', currentPeriodEnd: null }).paid).toBe(false);
  });

  it('says when a healthy plan renews', () => {
    const summary = summarizePlan({ plan: 'pro', status: 'active', currentPeriodEnd: '2026-09-03T00:00:00Z' });
    expect(summary.note).toMatch(/^Renews /);
    expect(summary.needsAttention).toBe(false);
  });

  /** The one case the user must not miss: it's still Pro today, and it won't
   * be for long. A badge alone would read as "everything is fine". */
  it('flags a failed payment', () => {
    const summary = summarizePlan({ plan: 'pro', status: 'past_due', currentPeriodEnd: null });
    expect(summary.needsAttention).toBe(true);
    expect(summary.note).toContain('Payment failed');
  });

  /** Cancelled but still inside the paid period: the plan is on its way out,
   * so it stops counting as paid — a gate reading `paid` shouldn't keep
   * letting someone through on a subscription they ended. */
  it('stops treating a cancelled plan as paid, and says when it ends', () => {
    const summary = summarizePlan({ plan: 'pro', status: 'canceled', currentPeriodEnd: '2026-09-03T00:00:00Z' });
    expect(summary.paid).toBe(false);
    expect(summary.note).toMatch(/^Access ends /);
    expect(summary.needsAttention).toBe(true);
  });

  it('treats a trial as the paid plan it is', () => {
    const summary = summarizePlan({ plan: 'pro', status: 'trialing', currentPeriodEnd: '2026-09-03T00:00:00Z' });
    expect(summary.paid).toBe(true);
    expect(summary.note).toMatch(/^Trial ends /);
  });

  /** Better no line at all than the words "Invalid Date" on someone's account
   * screen. */
  it('says nothing rather than printing an unreadable date', () => {
    expect(summarizePlan({ plan: 'pro', status: 'active', currentPeriodEnd: 'not-a-date' }).note).toBeNull();
    expect(summarizePlan({ plan: 'pro', status: 'active', currentPeriodEnd: null }).note).toBeNull();
  });
});
