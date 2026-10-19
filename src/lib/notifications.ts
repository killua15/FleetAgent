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

/** The list as the AI sorted it: one group per category that has anything,
 * most urgent first inside each, then whatever it hasn't sorted yet. */
export function groupByCategory(list: AppNotification[]): { id: AiCategory | 'unsorted'; items: AppNotification[] }[] {
  const byPriority = (a: AppNotification, b: AppNotification) =>
    (a.aiPriority ?? 9) - (b.aiPriority ?? 9) || b.createdAt.localeCompare(a.createdAt);
  const groups: { id: AiCategory | 'unsorted'; items: AppNotification[] }[] = AI_CATEGORIES.map((c) => ({
    id: c.id,
    items: list.filter((n) => n.aiCategory === c.id).sort(byPriority),
  }));
  const known = new Set<string>(AI_CATEGORIES.map((c) => c.id));
  groups.push({ id: 'unsorted', items: list.filter((n) => !n.aiCategory || !known.has(n.aiCategory)) });
  return groups.filter((g) => g.items.length > 0);
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
