import { useEffect, useMemo, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { KIND_META, ago, kindCounts, setNotificationsOpen, targetRoute, useNotificationsOpen } from '../lib/notifications';
import { useNavigation } from '../nav/NavigationProvider';
import type { AppNotification, NotificationKind } from '../types/notification';
import { toastError } from './ErrorOverlay';
import { Button } from './ui/button';
import styles from './NotificationsPanel.module.css';

/**
 * Everything Hangar notified about, inside Hangar — for when the OS
 * notifications never showed (Do Not Disturb, notifications off).
 *
 * A panel over the right edge of whatever screen you're on, grouped by what
 * the notification is about. Clicking one takes you there and marks it read.
 */
export function NotificationsPanel() {
  const open = useNotificationsOpen();
  const { navigate } = useNavigation();
  const [list, setList] = useState<AppNotification[]>([]);
  const [kind, setKind] = useState<NotificationKind | 'all'>('all');
  const [unreadOnly, setUnreadOnly] = useState(false);

  const load = () =>
    invoke<AppNotification[]>('list_notifications')
      .then(setList)
      .catch((e) => toastError(String(e)));

  useEffect(() => {
    if (!open) return;
    load();
    const added = listen('notification-added', load);
    const changed = listen('notifications-changed', load);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setNotificationsOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => {
      added.then((f) => f());
      changed.then((f) => f());
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const counts = useMemo(() => kindCounts(list), [list]);
  const shown = list.filter((n) => (kind === 'all' || n.kind === kind) && (!unreadOnly || !n.readAt));
  const unreadShown = shown.filter((n) => !n.readAt).map((n) => n.id);

  if (!open) return null;

  const markRead = (ids: string[] | null) => invoke('mark_notifications_read', { ids }).catch((e) => toastError(String(e)));

  const go = (n: AppNotification) => {
    if (!n.readAt) markRead([n.id]);
    const route = targetRoute(n);
    if (route) {
      navigate(route.screen, route.params);
      setNotificationsOpen(false);
    }
  };

  const clear = () =>
    invoke('clear_notifications', { kind: kind === 'all' ? null : kind })
      .then(load)
      .catch((e) => toastError(String(e)));

  return (
    <>
      <div className={styles.scrim} onClick={() => setNotificationsOpen(false)} />
      <aside className={styles.panel} aria-label="Notifications">
        <div className={styles.head}>
          <span className={styles.title}>Notifications</span>
          <span className={styles.spacer} />
          <label className={styles.unreadToggle}>
            <input type="checkbox" checked={unreadOnly} onChange={(e) => setUnreadOnly(e.target.checked)} />
            unread
          </label>
          <Button size="sm" variant="ghost" onClick={() => setNotificationsOpen(false)} title="Close (Esc)">
            ✕
          </Button>
        </div>

        <div className={styles.kinds}>
          <button className={kind === 'all' ? styles.kindOn : styles.kindOff} onClick={() => setKind('all')}>
            All <span className={styles.count}>{list.filter((n) => !n.readAt).length || list.length}</span>
          </button>
          {counts.map((c) => (
            <button key={c.kind} className={kind === c.kind ? styles.kindOn : styles.kindOff} onClick={() => setKind(c.kind)}>
              {KIND_META[c.kind].icon} {KIND_META[c.kind].label}
              <span className={c.unread > 0 ? styles.countUnread : styles.count}>{c.unread > 0 ? c.unread : c.total}</span>
            </button>
          ))}
        </div>

        <div className={styles.actions}>
          <Button size="sm" variant="outline" disabled={unreadShown.length === 0} onClick={() => markRead(kind === 'all' ? null : unreadShown)}>
            Mark read
          </Button>
          <Button size="sm" variant="ghost" disabled={shown.length === 0} onClick={clear}>
            Clear {kind === 'all' ? 'all' : KIND_META[kind].label.toLowerCase()}
          </Button>
        </div>

        <div className={styles.list}>
          {shown.length === 0 && (
            <span className={styles.empty}>{list.length === 0 ? 'Nothing yet. Notifications land here even with Do Not Disturb on.' : 'Nothing here.'}</span>
          )}
          {shown.map((n) => (
            <button key={n.id} className={n.readAt ? styles.item : styles.itemUnread} onClick={() => go(n)} title={targetRoute(n) ? 'Go there' : undefined}>
              <span className={styles.itemIcon}>{KIND_META[n.kind]?.icon ?? '•'}</span>
              <span className={styles.itemText}>
                <span className={styles.itemTitle}>{n.title}</span>
                <span className={styles.itemBody}>{n.body}</span>
              </span>
              <span className={styles.itemAgo}>{ago(n.createdAt)}</span>
            </button>
          ))}
        </div>
      </aside>
    </>
  );
}
