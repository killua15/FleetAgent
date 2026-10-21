import { useEffect, useMemo, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import {
  AI_CATEGORIES,
  KIND_META,
  ago,
  groupByCategory,
  kindCounts,
  setNotificationsOpen,
  sortedSummary,
  targetRoute,
  useNotificationsOpen,
  type CategoryGroupId,
} from '../lib/notifications';
import { useNavigation } from '../nav/NavigationProvider';
import type { Account, Provider } from '../types/account';
import type { AppNotification, NotificationKind } from '../types/notification';
import { AccountPickerButton } from './AccountPickerButton';
import { toastError } from './ErrorOverlay';
import { Button } from './ui/button';
import styles from './NotificationsPanel.module.css';

/** The providers whose accounts run Claude Code, which is what classifying
 * runs on — Kimi and custom endpoints are Claude Code pointed elsewhere. */
const CLASSIFY_PROVIDERS: Provider[] = ['claude', 'kimi', 'custom'];

type View = 'type' | 'ai';

/** The piles that start folded: worth a count, not a screenful. */
const FOLDABLE: CategoryGroupId[] = ['noise', 'seen'];

/** Per-viewer conveniences — storage may be unavailable. */
function stored(key: string): string | null {
  try {
    return localStorage.getItem(`hangar.notifications.${key}`);
  } catch {
    return null;
  }
}
function store(key: string, value: string) {
  try {
    localStorage.setItem(`hangar.notifications.${key}`, value);
  } catch {
    // Not remembered; still applied.
  }
}

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
  const [view, setView] = useState<View>(() => (stored('view') === 'ai' ? 'ai' : 'type'));
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [classifyAccount, setClassifyAccount] = useState<string | null>(() => stored('classifyAccount'));
  const [classifying, setClassifying] = useState(false);
  const [classifyNote, setClassifyNote] = useState<{ error: boolean; text: string } | null>(null);
  const [unfolded, setUnfolded] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(FOLDABLE.map((id) => [id, stored(`open.${id}`) === '1'])),
  );
  const listRef = useRef<HTMLDivElement>(null);

  const load = () =>
    invoke<AppNotification[]>('list_notifications')
      .then(setList)
      .catch((e) => toastError(String(e)));

  useEffect(() => {
    if (!open) return;
    load();
    invoke<Account[]>('list_accounts').then(setAccounts).catch(() => setAccounts([]));
    // A poll that finds several PRs fires a burst of events; one read covers it.
    let pending: number | undefined;
    const reload = () => {
      window.clearTimeout(pending);
      pending = window.setTimeout(load, 400);
    };
    const added = listen('notification-added', reload);
    const changed = listen('notifications-changed', reload);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setNotificationsOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.clearTimeout(pending);
      added.then((f) => f());
      changed.then((f) => f());
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);

  const counts = useMemo(() => kindCounts(list), [list]);
  const shown = list.filter((n) => (kind === 'all' || n.kind === kind) && (!unreadOnly || !n.readAt));
  const unreadShown = shown.filter((n) => !n.readAt).map((n) => n.id);
  const seenShown = shown.filter((n) => n.readAt).length;
  const groups = view === 'ai' ? groupByCategory(shown) : [];
  const unsorted = list.filter((n) => !n.readAt && !n.aiCategory).length;
  const rememberedAccount = accounts.find((a) => a.id === classifyAccount && CLASSIFY_PROVIDERS.includes(a.provider)) ?? null;

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

  const pickView = (next: View) => {
    setView(next);
    store('view', next);
  };

  const toggleFold = (id: CategoryGroupId) => {
    const next = !unfolded[id];
    setUnfolded((u) => ({ ...u, [id]: next }));
    store(`open.${id}`, next ? '1' : '0');
  };

  const classify = async (accountId: string) => {
    setClassifyAccount(accountId);
    store('classifyAccount', accountId);
    setClassifying(true);
    setClassifyNote(null);
    // What was waiting to be sorted, so the note can say where each one went.
    const before = new Set(list.filter((n) => !n.readAt && !n.aiCategory).map((n) => n.id));
    try {
      const outcome = await invoke<{ classified: number; remaining: number }>('classify_notifications', { accountId, ids: null });
      const fresh = await invoke<AppNotification[]>('list_notifications');
      setList(fresh);
      const landed = fresh.filter((n) => before.has(n.id) && n.aiCategory).map((n) => n.aiCategory);
      setClassifyNote({
        error: false,
        text:
          outcome.classified === 0
            ? 'Nothing new to sort.'
            : `${sortedSummary(landed)}${outcome.remaining > 0 ? ` — ${outcome.remaining} more on the next press` : ''}.`,
      });
      pickView('ai');
      // Back to the top, where what needs you now is.
      listRef.current?.scrollTo({ top: 0 });
    } catch (e) {
      // In the panel, next to the button that failed, not a toast that's
      // gone before you've read which account it was.
      setClassifyNote({ error: true, text: String(e) });
    } finally {
      setClassifying(false);
    }
  };

  const row = (n: AppNotification) => (
    <button key={n.id} className={n.readAt ? styles.item : styles.itemUnread} onClick={() => go(n)} title={targetRoute(n) ? 'Go there' : undefined}>
      <span className={styles.itemIcon}>{KIND_META[n.kind]?.icon ?? '•'}</span>
      <span className={styles.itemText}>
        <span className={styles.itemTitle}>
          {view === 'ai' && n.aiPriority === 1 && <span className={styles.urgent}>!</span>}
          {n.title}
        </span>
        <span className={styles.itemBody}>{n.body}</span>
        {view === 'ai' && n.aiReason && <span className={styles.itemReason}>✨ {n.aiReason}</span>}
      </span>
      <span className={styles.itemAgo}>{ago(n.createdAt)}</span>
    </button>
  );

  const clear = (readOnly: boolean) =>
    invoke('clear_notifications', { kind: kind === 'all' ? null : kind, readOnly })
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
          <Button size="sm" variant="ghost" disabled={seenShown === 0} onClick={() => clear(true)} title="Delete the ones you've already seen; what's unread stays">
            Clear seen{seenShown > 0 ? ` (${seenShown})` : ''}
          </Button>
          <Button size="sm" variant="ghost" disabled={shown.length === 0} onClick={() => clear(false)}>
            Clear {kind === 'all' ? 'all' : KIND_META[kind].label.toLowerCase()}
          </Button>
        </div>

        <div className={styles.actions}>
          <div className={styles.viewToggle}>
            <button className={view === 'type' ? styles.viewOn : styles.viewOff} onClick={() => pickView('type')} title="Newest first">
              Latest
            </button>
            <button className={view === 'ai' ? styles.viewOn : styles.viewOff} onClick={() => pickView('ai')} title="Grouped by what the AI made of them">
              By urgency
            </button>
          </div>
          <span className={styles.spacer} />
          {rememberedAccount ? (
            <>
              <Button
                size="sm"
                variant="outline"
                disabled={classifying || unsorted === 0}
                onClick={() => classify(rememberedAccount.id)}
                title={`Sort the ${unsorted} unread ones with ${rememberedAccount.name}`}
              >
                {classifying ? 'sorting…' : `✨ Classify${unsorted > 0 ? ` (${unsorted})` : ''}`}
              </Button>
              <AccountPickerButton
                label={rememberedAccount.name}
                title="Classify with another account"
                disabled={classifying}
                unavailable={(a) => (CLASSIFY_PROVIDERS.includes(a.provider) ? null : 'Classifying runs on Claude Code — this account runs another CLI')}
                onPick={classify}
              />
            </>
          ) : (
            <AccountPickerButton
              label={`✨ Classify${unsorted > 0 ? ` (${unsorted})` : ''}`}
              title="Let an AI sort the unread ones into needs you / review / FYI / noise"
              busy={classifying}
              busyLabel="sorting…"
              disabled={unsorted === 0}
              unavailable={(a) => (CLASSIFY_PROVIDERS.includes(a.provider) ? null : 'Classifying runs on Claude Code — this account runs another CLI')}
              onPick={classify}
            />
          )}
        </div>
        {classifyNote && <span className={classifyNote.error ? styles.noteError : styles.note}>{classifyNote.text}</span>}

        <div className={styles.list} ref={listRef}>
          {shown.length === 0 && (
            <span className={styles.empty}>{list.length === 0 ? 'Nothing yet. Notifications land here even with Do Not Disturb on.' : 'Nothing here.'}</span>
          )}
          {view === 'type' && shown.map(row)}
          {view === 'ai' &&
            groups.map((group) => {
              const meta = AI_CATEGORIES.find((c) => c.id === group.id);
              const unreadInGroup = group.items.filter((n) => !n.readAt).map((n) => n.id);
              const foldable = FOLDABLE.includes(group.id);
              const folded = foldable && !unfolded[group.id];
              const label = meta ? `${meta.icon} ${meta.label}` : group.id === 'seen' ? '✓ Seen' : 'Not sorted yet';
              return (
                <div key={group.id} className={styles.group}>
                  <div className={styles.groupHead}>
                    {foldable ? (
                      <button className={styles.groupFold} onClick={() => toggleFold(group.id)} title={folded ? 'Show them' : 'Fold them away'}>
                        {folded ? '▸' : '▾'} {label}
                      </button>
                    ) : (
                      <span>{label}</span>
                    )}
                    <span className={styles.count}>{group.items.length}</span>
                    <span className={styles.spacer} />
                    {group.id === 'noise' && unreadInGroup.length > 0 && (
                      <button className={styles.groupAction} onClick={() => markRead(unreadInGroup)}>
                        mark all read
                      </button>
                    )}
                    {group.id === 'seen' && (
                      <button className={styles.groupAction} onClick={() => clear(true)} title="Delete the ones you've already seen">
                        clear
                      </button>
                    )}
                  </div>
                  {!folded && group.items.map(row)}
                </div>
              );
            })}
        </div>
      </aside>
    </>
  );
}
