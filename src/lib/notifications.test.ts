import { describe, expect, it } from 'vitest';
import type { AppNotification } from '../types/notification';
import { ago, groupByCategory, kindCounts, sortedSummary, targetRoute } from './notifications';

const n = (over: Partial<AppNotification>): AppNotification => ({
  id: 'x',
  kind: 'agent',
  title: 't',
  body: 'b',
  target: null,
  createdAt: new Date().toISOString(),
  readAt: null,
  aiCategory: null,
  aiPriority: null,
  aiReason: null,
  ...over,
});

/** The panel is read with Do Not Disturb on — the groups and where a click
 * lands are the whole interface. */
describe('notification panel helpers', () => {
  it('counts per kind in display order, unread apart, and only kinds that have any', () => {
    const list = [n({ kind: 'pr' }), n({ kind: 'approval', readAt: '2026-10-17T00:00:00Z' }), n({ kind: 'pr', readAt: 'x' })];
    expect(kindCounts(list)).toEqual([
      { kind: 'approval', total: 1, unread: 0 },
      { kind: 'pr', total: 2, unread: 1 },
    ]);
  });

  it('sends each target to its screen', () => {
    expect(targetRoute(n({ target: { kind: 'terminal', id: 't1' } }))).toEqual({ screen: 'terminals', params: { focusTerminalId: 't1' } });
    expect(targetRoute(n({ target: { kind: 'session', id: 's1' } }))).toEqual({ screen: 'agent-panel', params: { sessionId: 's1' } });
    expect(targetRoute(n({ target: { kind: 'pr', id: 'a/b#1' } }))?.screen).toBe('pr-reviews');
    expect(targetRoute(n({ kind: 'task', target: null }))).toBeNull();
  });

  it('says how long ago in the units you scan', () => {
    const now = new Date('2026-10-17T12:00:00Z');
    expect(ago('2026-10-17T11:59:30Z', now)).toBe('now');
    expect(ago('2026-10-17T11:55:00Z', now)).toBe('5m');
    expect(ago('2026-10-17T09:00:00Z', now)).toBe('3h');
    expect(ago('2026-10-15T12:00:00Z', now)).toBe('2d');
  });
});

describe('groupByCategory', () => {
  const n = (id: string, aiCategory: string | null, aiPriority: number | null, createdAt = '2026-10-17T10:00:00Z', readAt: string | null = null) =>
    ({ id, kind: 'pr', title: id, body: '', target: null, createdAt, readAt, aiCategory, aiPriority, aiReason: null }) as AppNotification;

  it('reads top to bottom: needs you, review, FYI, unsorted, noise, then the seen ones', () => {
    const groups = groupByCategory([
      n('a', 'noise', 3),
      n('b', 'action_required', 2),
      n('c', null, null),
      n('d', 'action_required', 1),
      n('e', 'bogus', 1),
      n('f', 'fyi', 2),
      n('g', 'review', 2),
      n('h', 'action_required', 1, '2026-10-17T09:00:00Z', 'read'),
      n('i', null, null, '2026-10-17T11:00:00Z', 'read'),
    ]);
    expect(groups.map((g) => g.id)).toEqual(['action_required', 'review', 'fyi', 'unsorted', 'noise', 'seen']);
    expect(groups[0].items.map((i) => i.id)).toEqual(['d', 'b']);
    expect(groups[3].items.map((i) => i.id)).toEqual(['c', 'e']);
    expect(groups[5].items.map((i) => i.id)).toEqual(['i', 'h']);
  });

  it('keeps read ones out of every pile, even an urgent one', () => {
    const groups = groupByCategory([n('a', 'action_required', 1, undefined, 'read'), n('b', null, null, undefined, 'read')]);
    expect(groups.map((g) => g.id)).toEqual(['seen']);
  });

  it('breaks a priority tie by the newest', () => {
    const groups = groupByCategory([n('old', 'review', 2, '2026-10-17T08:00:00Z'), n('new', 'review', 2, '2026-10-17T12:00:00Z')]);
    expect(groups[0].items.map((i) => i.id)).toEqual(['new', 'old']);
  });
});

describe('sortedSummary', () => {
  it('says where a press put things, in pile order', () => {
    expect(sortedSummary(['noise', 'action_required', 'fyi', 'action_required'])).toBe('Sorted 4 — 2 need you, 1 FYI, 1 noise');
    expect(sortedSummary([])).toBe('Sorted 0');
  });
});
