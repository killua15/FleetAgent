/**
 * What to say about someone's plan.
 *
 * Here rather than in the screen for the reason `access.ts` is: given an
 * entitlement, the answer is a lookup with a handful of cases — no session, no
 * network, no component — and those cases are worth pinning down before
 * anything is built on top of them. The account section only paints what this
 * returns.
 */

import type { Entitlement } from '../lib/supabase';

/** The plans the `subscriptions` table can hold today. Renaming any of these is
 * a change to that table first and to this second — the value is written
 * server-side, so a client that invents its own vocabulary just stops matching.
 */
export const PLAN_LABEL: Record<Entitlement['plan'], string> = {
  free: 'Free',
  pro: 'Pro',
  team: 'Team',
};

export interface PlanSummary {
  /** What to show in the badge. */
  label: string;
  /** Whether this plan is a paid one — what feature gates ask about. */
  paid: boolean;
  /** One line under the badge, or null when there is nothing worth adding.
   * Empty for a healthy paid plan with no renewal date and for free. */
  note: string | null;
  /** Whether the state needs attention rather than just describing itself: a
   * payment that failed, a subscription being cancelled. */
  needsAttention: boolean;
}

/**
 * `null` means no subscription row at all, which is not an error — it is what
 * every account looks like before it ever pays for anything. Saying "Free" is
 * both true and less alarming than an empty space.
 */
export function summarizePlan(entitlement: Entitlement | null): PlanSummary {
  if (!entitlement) {
    return { label: PLAN_LABEL.free, paid: false, note: null, needsAttention: false };
  }

  const label = PLAN_LABEL[entitlement.plan] ?? entitlement.plan;
  const paid = entitlement.plan !== 'free';
  const status = entitlement.status?.toLowerCase() ?? '';

  // Stripe's vocabulary, which is what ends up in the column. `active` and
  // `trialing` are the healthy ones; everything else is worth reading.
  if (status === 'past_due' || status === 'unpaid') {
    return { label, paid, note: 'Payment failed — the plan will lapse if it stays unpaid.', needsAttention: true };
  }
  if (status === 'canceled' || status === 'cancelled') {
    return { label, paid: false, note: renewalNote(entitlement.currentPeriodEnd, 'Access ends'), needsAttention: true };
  }
  if (status === 'trialing') {
    return { label, paid, note: renewalNote(entitlement.currentPeriodEnd, 'Trial ends'), needsAttention: false };
  }

  return { label, paid, note: renewalNote(entitlement.currentPeriodEnd, 'Renews'), needsAttention: false };
}

/** `Renews 3 Sep 2026`, or nothing when the date is missing or unreadable —
 * a half-parsed date printed as "Invalid Date" is worse than silence. */
function renewalNote(iso: string | null, verb: string): string | null {
  if (!iso) return null;
  const when = new Date(iso);
  if (Number.isNaN(when.getTime())) return null;
  return `${verb} ${when.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })}`;
}
