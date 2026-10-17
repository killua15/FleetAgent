import { useSyncExternalStore } from 'react';
import type { ScreenName } from '../nav/NavigationProvider';
import type { AppNotification, NotificationKind } from '../types/notification';

/** The groups of the notification panel, in the order they're shown. */
export const KIND_META: Record<NotificationKind, { label: string; icon: string }> = {
  approval: { label: 'Approvals', icon: '✋' },
  agent: { label: 'Agents', icon: '◐' },
  pr: { label: 'PRs', icon: '⑂' },
  task: { label: 'Tasks', icon: '☑' },
  account: { label: 'Accounts', icon: '⚠' },
  update: { label: 'Updates', icon: '↑' },
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

/** Where clicking a notification goes: a screen and its params, or nowhere. */
export function targetRoute(n: AppNotification): { screen: ScreenName; params?: Record<string, unknown> } | null {
  const t = n.target;
  if (!t) return n.kind === 'pr' ? { screen: 'pr-reviews' } : null;
  switch (t.kind) {
    case 'terminal':
      return { screen: 'terminals', params: { focusTerminalId: t.id } };
    case 'session':
      return { screen: 'agent-panel', params: { sessionId: t.id } };
    case 'pr':
      return { screen: 'pr-reviews' };
    case 'account':
      return { screen: 'account-list' };
    case 'update':
      return { screen: 'general-settings' };
    case 'task':
      return { screen: 'task-board' };
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
