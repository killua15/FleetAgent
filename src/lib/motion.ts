import { useLayoutEffect, type RefObject } from 'react';

/** Talmo's easing curve, mirrored from `--ease` in tokens.css. WAAPI can't read
 * a CSS custom property, so it lives here too. */
export const EASE = 'cubic-bezier(0.22, 1, 0.36, 1)';
/** GSAP's `power3.out`, which Talmo uses for its reveals. */
export const EASE_OUT = 'cubic-bezier(0.215, 0.61, 0.355, 1)';
export const DURATION_MS = 420;

export function prefersReducedMotion(): boolean {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
}

/** The marker that tells the indicator which child is the current one. Covers
 * every way Hangar marks "selected": nav (`aria-current`), toggles
 * (`aria-pressed`), tabs (`aria-selected`) and an explicit `data-active`. */
const ACTIVE_SELECTOR =
  '[data-active="true"], [aria-current="page"], [aria-pressed="true"], [aria-selected="true"]';

/** Where each named indicator last stood. The top bar and the settings tabs
 * are rendered by each screen, so navigating unmounts the old pill and mounts
 * a new one — without this the new one appears already in place and the
 * slide Talmo is known for never happens. */
const lastBoxes = new Map<string, Box>();

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
  /** Names the indicator so a remounted one slides from where the last one
   * was, instead of appearing in place. */
  memoryKey?: string,
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
      const remembered = memoryKey ? lastBoxes.get(memoryKey) : undefined;
      const from: Box | null = wasPlaced
        ? {
            x: Number(pill.dataset.x),
            y: Number(pill.dataset.y),
            w: Number(pill.dataset.w),
            h: Number(pill.dataset.h),
          }
        : (remembered ?? null);
      if (memoryKey) lastBoxes.set(memoryKey, to);

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

    // A remounted indicator animates its first placement too — from the spot
    // its predecessor was in.
    place(true);

    // Labels collapse at breakpoints and fonts swap in late: both move the
    // active item without a change of `activeKey`.
    const ro = new ResizeObserver(() => place(false));
    ro.observe(container);
    for (const child of Array.from(container.children)) ro.observe(child);
    return () => ro.disconnect();
  }, [containerRef, pillRef, activeKey, memoryKey]);
}

/** What a screen reveals on arrival: its cards, rows and headers — matched by
 * shadcn slot or by CSS-module class name, since screens don't share
 * components for these. */
const REVEAL_SELECTOR = [
  '[data-reveal]',
  '[data-slot="card"]',
  '[data-slot="page-header"]',
  '[class*="_card"]',
  '[class*="_row"]',
  '[class*="_tile"]',
  '[class*="_section"]',
  '[class*="_header"]',
  '[class*="_head_"]',
  '[class*="_group"]',
  '[class*="_column"]',
  '[class*="_item"]',
].join(', ');

const REVEAL_MAX = 28;

/**
 * Talmo's arrival: the screen's blocks rise into place one after another
 * (opacity 0 → 1, 12px up, 40ms apart). Only outermost matches move — a card
 * rises with its rows, not under them — only what's on screen, and never more
 * than a few dozen, so a long list doesn't spend a second arriving. What's on
 * screen past that cap shows at once and is marked as arrived.
 */
export function revealChildren(root: HTMLElement, seen: WeakSet<Element> = new WeakSet()) {
  if (prefersReducedMotion() || typeof root.animate !== 'function') return;
  const view = window.innerHeight;
  const all = Array.from(root.querySelectorAll<HTMLElement>(REVEAL_SELECTOR));
  const picked: HTMLElement[] = [];
  for (const el of all) {
    if (picked.some((p) => p.contains(el))) continue;
    // Already arrived (an earlier pass), or inside something that did.
    if (seen.has(el)) continue;
    let inSeen = false;
    for (let a = el.parentElement; a && a !== root; a = a.parentElement) if (seen.has(a)) inSeen = true;
    if (inSeen) continue;
    // Skip the top bar: it stays put while the screen arrives under it.
    if (el.closest('[data-no-reveal]')) continue;
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0 || r.top > view || r.bottom < 0) continue;
    // Past the cap it is already on screen, so it counts as arrived: a later
    // pass must not pull it back to opacity 0 and play it a second time.
    if (picked.length >= REVEAL_MAX) {
      seen.add(el);
      continue;
    }
    picked.push(el);
  }
  picked.forEach((el, i) => {
    seen.add(el);
    el.animate(
      [
        { opacity: 0, transform: 'translateY(12px)' },
        { opacity: 1, transform: 'none' },
      ],
      { duration: 550, delay: i * 40, easing: EASE_OUT, fill: 'backwards' },
    );
  });
}

/**
 * Plays `revealChildren` on every navigation. Screens load their data after
 * they mount, so one pass would find an empty list: a few passes in the first
 * half second catch what arrives late, each element animating only once.
 */
export function useScreenReveal(screenKey: unknown, getRoot: () => HTMLElement | null) {
  useLayoutEffect(() => {
    const seen = new WeakSet<Element>();
    const run = () => {
      const root = getRoot();
      if (root) revealChildren(root, seen);
    };
    const raf = requestAnimationFrame(run);
    const timers = [180, 450].map((ms) => window.setTimeout(run, ms));
    return () => {
      cancelAnimationFrame(raf);
      timers.forEach((t) => window.clearTimeout(t));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [screenKey]);
}
