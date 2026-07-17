import type { SessionView } from '../types/session';

export interface SessionTreeRow {
  session: SessionView;
  depth: number;
}

/** Orders a flat session list so each subsession immediately follows its
 * parent, with a depth for indentation — parent-then-children, depth-first.
 * A subsession whose parent isn't in the same list (e.g. it moved to a
 * different account) just renders as its own root at depth 0. */
export function buildSessionTree(sessions: SessionView[]): SessionTreeRow[] {
  const ids = new Set(sessions.map((s) => s.id));
  const childrenByParent = new Map<string, SessionView[]>();
  const roots: SessionView[] = [];
  for (const s of sessions) {
    if (s.parentSessionId && ids.has(s.parentSessionId)) {
      const list = childrenByParent.get(s.parentSessionId) ?? [];
      list.push(s);
      childrenByParent.set(s.parentSessionId, list);
    } else {
      roots.push(s);
    }
  }
  const result: SessionTreeRow[] = [];
  const visit = (session: SessionView, depth: number) => {
    result.push({ session, depth });
    for (const child of childrenByParent.get(session.id) ?? []) {
      visit(child, depth + 1);
    }
  };
  roots.forEach((r) => visit(r, 0));
  return result;
}

/** Counts a session plus every descendant subsession (recursively) within a
 * flat list — used so a delete confirmation can warn "this also removes N
 * subsessions" instead of silently cascading. */
export function countSessionSubtree(sessions: SessionView[], rootId: string): number {
  const childrenByParent = new Map<string, string[]>();
  for (const s of sessions) {
    if (!s.parentSessionId) continue;
    const list = childrenByParent.get(s.parentSessionId) ?? [];
    list.push(s.id);
    childrenByParent.set(s.parentSessionId, list);
  }
  let count = 0;
  const visit = (id: string) => {
    for (const childId of childrenByParent.get(id) ?? []) {
      count += 1;
      visit(childId);
    }
  };
  visit(rootId);
  return count;
}
