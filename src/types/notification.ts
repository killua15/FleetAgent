/** Matches `notifications::kind` in Rust. */
export type NotificationKind = 'approval' | 'agent' | 'pr' | 'account' | 'update' | 'task';

/** Where clicking a notification takes you. Matches `notifications::Target`. */
export interface NotificationTarget {
  kind: 'terminal' | 'session' | 'pr' | 'account' | 'update' | 'task';
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
