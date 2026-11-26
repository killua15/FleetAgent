import { useLayoutEffect, type RefObject } from 'react';

/** Talmo's easing curve, mirrored from `--ease` in tokens.css. WAAPI can't read
 * a CSS custom property, so it lives here too. */
export const EASE = 'cubic-bezier(0.22, 1, 0.36, 1)';
export const DURATION_MS = 340;

export function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** The marker that tells the indicator which child is the current one. Covers
 * every way Hangar marks "selected": nav (`aria-current`), toggles
 * (`aria-pressed`), tabs (`aria-selected`) and an explicit `data-active`. */
const ACTIVE_SELECTOR =
  '[data-active="true"], [aria-current="page"], [aria-pressed="true"], [aria-selected="true"]';

interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
}

function measure(container: HTMLElement, item: HTMLElement): Box {
  const c = container.getBoundingClientRect();
  const r = item.getBoundingClientRect();
  return {
    x: r.left - c.left + container.scrollLeft,
    y: r.top - c.top + container.scrollTop,
    w: r.width,
    h: r.height,
  };
}

function frame(b: Box): Keyframe {
  return { transform: `translate(${b.x}px, ${b.y}px)`, width: `${b.w}px`, height: `${b.h}px` };
}

/**
 * The sliding pill behind the active item of a nav, segmented control or tab
 * bar — Talmo's `tnav__pill` / `seg__thumb` / `tabbar__pill`.
 *
 * The pill is an absolutely positioned element at the container's top-left;
 * this sizes and moves it onto whichever child carries an active marker.
 * The first placement and any resize jump straight there, and only a change of
 * the active item animates, so a window resize never sets the pill swimming.
 * With nothing active the pill fades out instead of sitting on a stale item.
 *
 * `activeKey` is whatever identifies the active item (a screen name, a value);
 * it is only a dependency, so the hook re-measures when it changes.
 */
export function useSlidingIndicator(
  containerRef: RefObject<HTMLElement | null>,
  pillRef: RefObject<HTMLElement | null>,
  activeKey: unknown,
) {
  useLayoutEffect(() => {
    const container = containerRef.current;
    const pill = pillRef.current;
    if (!container || !pill) return;

    const place = (animate: boolean) => {
      const item = container.querySelector<HTMLElement>(ACTIVE_SELECTOR);
      // Hidden (display: none at a breakpoint) items measure as 0×0.
      if (!item || item.offsetWidth === 0) {
        pill.style.opacity = '0';
        pill.dataset.placed = '';
        return;
      }
      const to = measure(container, item);
      const wasPlaced = pill.dataset.placed === 'true';
      const from: Box | null = wasPlaced
        ? {
            x: Number(pill.dataset.x),
            y: Number(pill.dataset.y),
            w: Number(pill.dataset.w),
            h: Number(pill.dataset.h),
          }
        : null;

      Object.assign(pill.style, frame(to) as Record<string, string>);
      pill.style.opacity = '1';
      pill.dataset.placed = 'true';
      pill.dataset.x = String(to.x);
      pill.dataset.y = String(to.y);
      pill.dataset.w = String(to.w);
      pill.dataset.h = String(to.h);

      const moved = from && (from.x !== to.x || from.y !== to.y || from.w !== to.w);
      if (animate && moved && !prefersReducedMotion() && typeof pill.animate === 'function') {
        pill.animate([frame(from), frame(to)], { duration: DURATION_MS, easing: EASE });
      }
    };

    place(true);

    // Labels collapse at breakpoints and fonts swap in late: both move the
    // active item without a change of `activeKey`.
    const ro = new ResizeObserver(() => place(false));
    ro.observe(container);
    for (const child of Array.from(container.children)) ro.observe(child);
    return () => ro.disconnect();
  }, [containerRef, pillRef, activeKey]);
}
