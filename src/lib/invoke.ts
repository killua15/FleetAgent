import { invoke as tauriInvoke, type InvokeArgs, type InvokeOptions } from '@tauri-apps/api/core';
import { useSyncExternalStore } from 'react';

// Drop-in replacement for `@tauri-apps/api/core`'s `invoke` that tracks how
// many calls are currently in flight, app-wide — the one thing every screen
// gets "for free" just by importing this instead of the real thing, so no
// invoke() call is ever silently unaccounted for even if nobody remembered
// to add a spinner at that specific call site.
let inFlight = 0;
const listeners = new Set<() => void>();

function notify() {
  for (const l of listeners) l();
}

export function invoke<T>(cmd: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  inFlight += 1;
  notify();
  return tauriInvoke<T>(cmd, args, options).finally(() => {
    inFlight -= 1;
    notify();
  });
}

function subscribe(onChange: () => void) {
  listeners.add(onChange);
  return () => listeners.delete(onChange);
}

function getSnapshot() {
  return inFlight;
}

/** Current count of in-flight `invoke()` calls made through this wrapper. */
export function useInFlightCount(): number {
  return useSyncExternalStore(subscribe, getSnapshot);
}
