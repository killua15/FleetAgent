/** Matches `notifications::kind` in Rust: what's worth a row. Quota, spend cap
 * and updates have their own banners and only reach the OS. */
export type NotificationKind = 'approval' | 'pr' | 'task' | 'agent';

/** Where clicking a notification takes you. Matches `notifications::Target`. */
export interface NotificationTarget {
  kind: 'terminal' | 'session' | 'pr' | 'task';
  id: string | null;
}

/** One row of Hangar's own notification list. Matches `AppNotification`. */
export interface AppNotification {
  id: string;
  kind: NotificationKind;
  title: string;
  body: string;
  target: NotificationTarget | null;
  createdAt: string;
  readAt: string | null;
  aiCategory: string | null;
  aiPriority: number | null;
  aiReason: string | null;
}
