import { useEffect, useRef, useState } from 'react';

/** Tracks an element's live width via `ResizeObserver` — used by the tree
 * screens (`TreeCanvas`/`TreeColumns`) to re-center their layout when the
 * window resizes, instead of freezing at whatever width was measured on
 * first render. */
export function useElementWidth<T extends HTMLElement>(initial = 1100): [React.RefObject<T | null>, number] {
  const ref = useRef<T>(null);
  const [width, setWidth] = useState(initial);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const w = entries[0]?.contentRect.width;
      if (w) setWidth(w);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}
