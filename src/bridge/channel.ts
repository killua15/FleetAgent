import type { RealtimeChannel } from '@supabase/supabase-js';
import { supabase } from '../lib/supabase';

/** Waits between attempts. A cold Realtime tenant (the project slept) took
 * ~25 s to accept joins; these cover that with room to spare. */
export const RETRY_DELAYS_MS = [1_000, 3_000, 8_000, 15_000];

type Status = 'SUBSCRIBED' | 'CHANNEL_ERROR' | 'TIMED_OUT' | 'CLOSED';

/** The slice of a Realtime channel this needs, so tests can fake it. */
export interface Subscribable {
  subscribe(callback: (status: Status, err?: Error) => void): unknown;
}

interface Options<C extends Subscribable> {
  /** Says which channel failed, in the error. */
  label: string;
  /** Drops a channel that failed to join. Defaults to `supabase.removeChannel`. */
  remove?: (channel: C) => Promise<unknown>;
  delays?: number[];
}

/**
 * Creates a channel with `make` and waits until it is really subscribed,
 * retrying a failed join with a fresh channel.
 *
 * A `CHANNEL_ERROR` or `TIMED_OUT` is often passing — Realtime waking up, a
 * network blip — and giving up on the first one is what left a machine deaf
 * until Hangar was restarted. `CLOSED` is not retried: it means we closed it.
 * The error names the status and Realtime's own reason, which used to be
 * dropped.
 */
export async function subscribeWithRetry<C extends Subscribable = RealtimeChannel>(make: () => C, options: Options<C>): Promise<C> {
  const remove = options.remove ?? ((channel: C) => supabase.removeChannel(channel as unknown as RealtimeChannel));
  const delays = options.delays ?? RETRY_DELAYS_MS;
  let last = '';

  for (let attempt = 0; ; attempt++) {
    const channel = make();
    const outcome = await new Promise<{ status: Status; reason?: string }>((resolve) => {
      let settled = false;
      channel.subscribe((status, err) => {
        if (settled || (status !== 'SUBSCRIBED' && status !== 'CHANNEL_ERROR' && status !== 'TIMED_OUT' && status !== 'CLOSED')) return;
        settled = true;
        resolve({ status, reason: err?.message });
      });
    });
    if (outcome.status === 'SUBSCRIBED') return channel;

    last = outcome.reason ? `${outcome.status} — ${outcome.reason}` : outcome.status;
    await remove(channel);
    if (outcome.status === 'CLOSED' || attempt >= delays.length) {
      throw new Error(`${options.label}: ${last}`);
    }
    await new Promise((resolve) => setTimeout(resolve, delays[attempt]));
  }
}
