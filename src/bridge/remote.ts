import type { RealtimeChannel } from '@supabase/supabase-js';
import { invoke } from '../lib/invoke';
import { supabase } from '../lib/supabase';
import type { Envelope } from './pump';
import { subscribeWithRetry } from './channel';

/** What `remote_open` hands back once Rust has opened an envelope. */
interface Opened {
  id: string;
  kind: Envelope['kind'];
  from: string;
  body: unknown;
}

/** The other machine's greeting. */
export interface RemoteHello {
  machineName?: string;
  commands: string[];
  /** `false`: the machine is there, but remote control is turned off on it. */
  remoteAccess?: boolean;
}

/** A failure the other machine reported, with the bridge's error code. */
export class RemoteError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}

/** Re-greets this often; the host forgets a machine after 75 s of silence. */
const HEARTBEAT_MS = 30_000;
const CALL_TIMEOUT_MS = 20_000;

interface Pending {
  resolve: (value: unknown) => void;
  reject: (error: Error) => void;
  timer: number;
}

/**
 * A connection to another machine of the account, from this one.
 *
 * It joins that machine's channel (`hangar:<id>`, which RLS opens to any
 * machine of the same account) and talks the bridge protocol to it, as the
 * phone does — except the key comes from both machines' published public keys
 * instead of a QR. Like `pump.ts`, it is a dumb pipe: Rust seals and opens.
 */
export class RemoteMachine {
  private channel: RealtimeChannel | null = null;
  private pending = new Map<string, Pending>();
  private listeners = new Map<string, Set<(data: unknown) => void>>();
  private heartbeat: number | null = null;
  /** Incoming envelopes are opened one after another, so terminal output is
   * drawn in the order it was sent even though opening each one is async. */
  private inbox: Promise<void> = Promise.resolve();
  hello: RemoteHello | null = null;

  constructor(readonly peerId: string) {}

  async connect(): Promise<RemoteHello> {
    if (!this.channel) {
      this.channel = await subscribeWithRetry(
        () => {
          const channel = supabase.channel(`hangar:${this.peerId}`, { config: { private: true, broadcast: { self: false } } });
          channel.on('broadcast', { event: 'msg' }, ({ payload }) => {
            this.inbox = this.inbox.then(() => this.receive(payload as Envelope)).catch(() => {});
          });
          return channel;
        },
        { label: 'no se pudo abrir el canal de la otra máquina' },
      );
      this.heartbeat = window.setInterval(() => void this.greet().catch(() => {}), HEARTBEAT_MS);
    }
    return this.greet();
  }

  /** Says hello; the answer says whether it is up and what it accepts. */
  async greet(): Promise<RemoteHello> {
    const hello = (await this.send('hello', { v: 1, device: 'Hangar' })) as RemoteHello;
    this.hello = hello;
    return hello;
  }

  /** Runs one of that machine's commands and returns its data. */
  async call<T>(name: string, args: Record<string, unknown> = {}): Promise<T> {
    const result = (await this.send('invoke', { name, args })) as { ok: boolean; data: T };
    return result.data;
  }

  /** Listens to one of that machine's events. Returns the unsubscribe. */
  on<T>(name: string, listener: (data: T) => void): () => void {
    const set = this.listeners.get(name) ?? new Set();
    set.add(listener as (data: unknown) => void);
    this.listeners.set(name, set);
    return () => set.delete(listener as (data: unknown) => void);
  }

  async close(): Promise<void> {
    if (this.heartbeat !== null) window.clearInterval(this.heartbeat);
    this.heartbeat = null;
    for (const call of this.pending.values()) {
      window.clearTimeout(call.timer);
      call.reject(new Error('conexión cerrada'));
    }
    this.pending.clear();
    if (this.channel) await supabase.removeChannel(this.channel);
    this.channel = null;
  }

  /**
   * One request, one answer, matched by id. **Never retried**: a command in
   * flight when the link drops may have run (TRD §9).
   */
  private async send(kind: 'hello' | 'invoke', body: unknown): Promise<unknown> {
    if (!this.channel) throw new Error('sin conexión con la otra máquina');
    const id = crypto.randomUUID();
    const envelope = await invoke<Envelope>('remote_seal', { peerId: this.peerId, kind, id, body });
    const answer = new Promise<unknown>((resolve, reject) => {
      const timer = window.setTimeout(() => {
        this.pending.delete(id);
        // Its own code: the screen explains the usual cause (remote access off
        // there, so nothing listens) instead of guessing from this text.
        reject(new RemoteError('TIMEOUT', 'la otra máquina no respondió'));
      }, CALL_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timer });
    });
    await this.channel.send({ type: 'broadcast', event: 'msg', payload: envelope });
    return answer;
  }

  private async receive(envelope: Envelope): Promise<void> {
    let opened: Opened | null;
    try {
      opened = await invoke<Opened | null>('remote_open', { envelope });
    } catch {
      // Could not be opened: not from that machine, or tampered with.
      return;
    }
    if (!opened || opened.from !== this.peerId) return;

    if (opened.kind === 'event') {
      const event = opened.body as { name: string; data: unknown };
      for (const listener of this.listeners.get(event.name) ?? []) listener(event.data);
      return;
    }

    const call = this.pending.get(opened.id);
    if (!call) return;
    this.pending.delete(opened.id);
    window.clearTimeout(call.timer);
    if (opened.kind === 'error') {
      const failure = opened.body as { code: string; message: string };
      call.reject(new RemoteError(failure.code, failure.message));
    } else {
      call.resolve(opened.body);
    }
  }
}

/** One connection per machine, shared by the Machines screen and its panes. */
const connections = new Map<string, RemoteMachine>();

export function remoteMachine(peerId: string): RemoteMachine {
  let machine = connections.get(peerId);
  if (!machine) {
    machine = new RemoteMachine(peerId);
    connections.set(peerId, machine);
  }
  return machine;
}

export async function closeRemoteMachine(peerId: string): Promise<void> {
  const machine = connections.get(peerId);
  connections.delete(peerId);
  await machine?.close();
}
