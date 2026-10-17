import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { toggleNotifications, useNotificationsOpen } from '../lib/notifications';
import { BellIcon } from './NavIcons';
import { IconTooltip } from './ui/tooltip';
import styles from './NotificationBell.module.css';

/** The unread count, kept current by the backend's `notifications-changed`. */
export function useUnreadNotifications(): number {
  const [unread, setUnread] = useState(0);
  useEffect(() => {
    invoke<number>('unread_notification_count').then(setUnread).catch(() => {});
    const unlisten = listen<number>('notifications-changed', (e) => setUnread(e.payload));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);
  return unread;
}

/** 🔔 with the unread count. `className` lets the top bar style it like its
 * other icon buttons; without it, it floats in the corner. */
export function NotificationBell({ className, activeClassName }: { className?: string; activeClassName?: string }) {
  const unread = useUnreadNotifications();
  const open = useNotificationsOpen();
  return (
    <IconTooltip label={unread > 0 ? `Notifications — ${unread} unread` : 'Notifications'}>
      <button
        className={(open ? activeClassName : className) ?? (open ? styles.floatingActive : styles.floating)}
        onClick={toggleNotifications}
        aria-label="Notifications"
        aria-pressed={open}
      >
        <BellIcon />
        {unread > 0 && <span className={styles.badge}>{unread > 99 ? '99+' : unread}</span>}
      </button>
    </IconTooltip>
  );
}
