import { useEffect, type RefObject } from 'react';

/** Closes a popover/dropdown on any click outside `ref`'s element — the
 * standard behavior every popover should have but none in this codebase did
 * (they only closed by re-clicking their own toggle button). Only attaches
 * the listener while `active`, so idle closed popovers cost nothing. */
export function useClickOutside(ref: RefObject<HTMLElement | null>, onOutside: () => void, active: boolean) {
  useEffect(() => {
    if (!active) return;
    const handler = (e: MouseEvent) => {
      const target = e.target as HTMLElement;
      // shadcn's Select portals its dropdown to document.body (Radix
      // Select.Portal), so it's never a DOM descendant of `ref` even when
      // it's visually nested inside the panel `ref` wraps — without this,
      // picking an option here reads as "clicked outside" and closes the
      // whole panel before the selection even applies.
      if (target.closest?.('[data-slot="select-content"]')) return;
      if (ref.current && !ref.current.contains(target)) onOutside();
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [active, onOutside, ref]);
}
