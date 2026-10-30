import { useSyncExternalStore } from 'react';
import type { ScreenName } from '../nav/NavigationProvider';
import type { AppNotification, NotificationKind } from '../types/notification';

/** The groups of the notification panel, in the order they're shown. */
export const KIND_META: Record<NotificationKind, { label: string; icon: string }> = {
  approval: { label: 'Approvals', icon: '✋' },
  pr: { label: 'PRs', icon: '⑂' },
  task: { label: 'Tasks', icon: '☑' },
  // The half-hourly summary of agents waiting, not one per turn.
  agent: { label: 'Agents', icon: '◐' },
};

export const KIND_ORDER = Object.keys(KIND_META) as NotificationKind[];

/** How many notifications, and how many unread, of each kind — only the kinds
 * that have any, in display order. */
export function kindCounts(list: AppNotification[]): { kind: NotificationKind; total: number; unread: number }[] {
  return KIND_ORDER.map((kind) => {
    const of = list.filter((n) => n.kind === kind);
    return { kind, total: of.length, unread: of.filter((n) => !n.readAt).length };
  }).filter((c) => c.total > 0);
}

/** The AI's four piles, in the order they're shown. */
export const AI_CATEGORIES = [
  { id: 'action_required', label: 'Needs you', icon: '⚡' },
  { id: 'review', label: 'Review', icon: '👁' },
  { id: 'fyi', label: 'FYI', icon: 'ℹ' },
  { id: 'noise', label: 'Noise', icon: '·' },
] as const;

export type AiCategory = (typeof AI_CATEGORIES)[number]['id'];

export type CategoryGroupId = AiCategory | 'unsorted' | 'seen';

/** The list as the AI sorted it, laid out for reading top to bottom: what needs
 * you, what to review, FYI, whatever it hasn't sorted yet, then noise — and
 * last, everything already seen, whatever its pile. Classifying only ever
 * looks at unread ones, so without that last group every read notification
 * would sit in "not sorted yet" as the biggest pile on screen.
 *
 * Inside a group: most urgent first, then newest. Groups with nothing in them
 * are left out. */
export function groupByCategory(list: AppNotification[]): { id: CategoryGroupId; items: AppNotification[] }[] {
  const newest = (a: AppNotification, b: AppNotification) => b.createdAt.localeCompare(a.createdAt);
  const byPriority = (a: AppNotification, b: AppNotification) => (a.aiPriority ?? 9) - (b.aiPriority ?? 9) || newest(a, b);
  const pending = list.filter((n) => !n.readAt);
  const known = new Set<string>(AI_CATEGORIES.map((c) => c.id));
  const pile = (id: AiCategory) => ({ id, items: pending.filter((n) => n.aiCategory === id).sort(byPriority) });
  const groups: { id: CategoryGroupId; items: AppNotification[] }[] = [
    pile('action_required'),
    pile('review'),
    pile('fyi'),
    { id: 'unsorted', items: pending.filter((n) => !n.aiCategory || !known.has(n.aiCategory)).sort(newest) },
    pile('noise'),
    { id: 'seen', items: list.filter((n) => n.readAt).sort(newest) },
  ];
  return groups.filter((g) => g.items.length > 0);
}

/** "Sorted 12 — 2 need you, 3 to review, 5 FYI, 2 noise": where a classify
 * press put things, from the categories of the ones it just sorted. */
export function sortedSummary(categories: (string | null)[]): string {
  const words: Record<AiCategory, string> = { action_required: 'need you', review: 'to review', fyi: 'FYI', noise: 'noise' };
  const parts = AI_CATEGORIES.map((c) => ({ n: categories.filter((x) => x === c.id).length, word: words[c.id] }))
    .filter((p) => p.n > 0)
    .map((p) => `${p.n} ${p.word}`);
  return `Sorted ${categories.length}${parts.length ? ` — ${parts.join(', ')}` : ''}`;
}

/** Where clicking a notification goes: a screen and its params, or nowhere. */
export function targetRoute(n: AppNotification): { screen: ScreenName; params?: Record<string, unknown> } | null {
  const t = n.target;
  if (!t) return n.kind === 'pr' ? { screen: 'pr-reviews' } : null;
  switch (t.kind) {
    case 'terminal':
      // No id: a summary of several panels — the terminals screen itself.
      return { screen: 'terminals', params: t.id ? { focusTerminalId: t.id } : undefined };
    case 'session':
      return { screen: 'agent-panel', params: { sessionId: t.id } };
    case 'pr':
      return { screen: 'pr-reviews' };
    case 'task':
      return { screen: 'terminals', params: { showTasks: true } };
    case 'factory':
      return { screen: 'factory' };
    default:
      return null;
  }
}

/** "now", "5m", "3h", "2d" — a list you scan, not a log you audit. */
export function ago(iso: string, now: Date = new Date()): string {
  const secs = Math.max(0, Math.round((now.getTime() - new Date(iso).getTime()) / 1000));
  if (secs < 60) return 'now';
  if (secs < 3600) return `${Math.floor(secs / 60)}m`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h`;
  return `${Math.floor(secs / 86400)}d`;
}

// ── Panel open/closed, shared by every bell and the panel itself ───────────
let panelOpen = false;
const openListeners = new Set<() => void>();
export function setNotificationsOpen(open: boolean) {
  panelOpen = open;
  openListeners.forEach((l) => l());
}
export function toggleNotifications() {
  setNotificationsOpen(!panelOpen);
}
export function useNotificationsOpen(): boolean {
  return useSyncExternalStore(
    (l) => {
      openListeners.add(l);
      return () => openListeners.delete(l);
    },
    () => panelOpen,
  );
}

// ── Whether a top bar is on screen, so the floating bell only shows where
//    there's no bar to carry one ───────────────────────────────────────────
let topBars = 0;
const barListeners = new Set<() => void>();
export function registerTopBar(): () => void {
  topBars += 1;
  barListeners.forEach((l) => l());
  return () => {
    topBars -= 1;
    barListeners.forEach((l) => l());
  };
}
export function useHasTopBar(): boolean {
  return useSyncExternalStore(
    (l) => {
      barListeners.add(l);
      return () => barListeners.delete(l);
    },
    () => topBars > 0,
  );
}
