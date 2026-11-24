import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../lib/supabase', () => ({ supabase: { removeChannel: vi.fn() } }));

import { subscribeWithRetry, type Subscribable } from './channel';

type Status = Parameters<Parameters<Subscribable['subscribe']>[0]>[0];

/** A channel that answers its subscribe with a scripted status. */
function scripted(statuses: Array<[Status, string?]>) {
  const made: Subscribable[] = [];
  const make = () => {
    const [status, reason] = statuses[made.length] ?? ['SUBSCRIBED'];
    const channel: Subscribable = {
      subscribe: (callback) => queueMicrotask(() => callback(status, reason ? new Error(reason) : undefined)),
    };
    made.push(channel);
    return channel;
  };
  return { make, made };
}

describe('subscribeWithRetry', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('retries a failed join with a fresh channel until it subscribes', async () => {
    const { make, made } = scripted([['CHANNEL_ERROR', 'tenant initializing'], ['TIMED_OUT']]);
    const remove = vi.fn(async () => {});
    const result = subscribeWithRetry(make, { label: 'test', remove, delays: [10, 20] });
    await vi.runAllTimersAsync();
    expect(await result).toBe(made[2]);
    expect(made).toHaveLength(3);
    expect(remove).toHaveBeenCalledTimes(2);
  });

  it('gives up after the last attempt, with Realtime’s reason in the message', async () => {
    const { make, made } = scripted([['CHANNEL_ERROR', 'Unauthorized'], ['CHANNEL_ERROR', 'Unauthorized'], ['CHANNEL_ERROR', 'Unauthorized']]);
    const result = subscribeWithRetry(make, { label: 'canal', remove: async () => {}, delays: [10, 20] });
    const assertion = expect(result).rejects.toThrow('canal: CHANNEL_ERROR — Unauthorized');
    await vi.runAllTimersAsync();
    await assertion;
    expect(made).toHaveLength(3);
  });

  it('does not retry a channel we closed', async () => {
    const { make, made } = scripted([['CLOSED']]);
    const result = subscribeWithRetry(make, { label: 'canal', remove: async () => {}, delays: [10, 20] });
    const assertion = expect(result).rejects.toThrow('canal: CLOSED');
    await vi.runAllTimersAsync();
    await assertion;
    expect(made).toHaveLength(1);
  });
});
