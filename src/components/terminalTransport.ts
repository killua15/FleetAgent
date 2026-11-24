import { Channel } from '@tauri-apps/api/core';
import { invoke } from '../lib/invoke';
import type { RemoteMachine } from '../bridge/remote';
import type { TerminalChunk } from '../types/terminal';

export interface PtySize {
  cols: number;
  rows: number;
}

/**
 * How a `TerminalPane` reaches its PTY: in this process, or on another machine
 * of the account over the bridge. The pane draws; this moves bytes.
 */
export interface TerminalTransport {
  /**
   * Starts the stream and returns the scrollback (base64). `onChunk` may fire
   * before this resolves; the pane holds those until the scrollback is drawn.
   * `size` is set when the PTY's size is owned elsewhere and the pane must
   * render at it instead of fitting.
   */
  attach(id: string, onChunk: (base64: string, size?: PtySize) => void): Promise<{ scrollback: string; size?: PtySize }>;
  detach(id: string): void;
  write(id: string, data: string): void;
  /** `null`: this pane cannot resize the PTY (the other machine owns it). */
  resize: ((id: string, size: PtySize) => void) | null;
  /** Dropped files become paths on this machine, meaningless on another. */
  acceptsFiles: boolean;
}

/** The PTY in this process — exactly what the pane always did. */
export const localTransport: TerminalTransport = {
  async attach(id, onChunk) {
    const channel = new Channel<TerminalChunk>();
    channel.onmessage = (chunk) => onChunk(chunk.base64);
    // `attach_terminal` installs the channel and returns the scrollback in one
    // atomic step, so nothing printed in between is lost.
    const scrollback = await invoke<string>('attach_terminal', { id, channel });
    return { scrollback };
  },
  detach(id) {
    invoke('detach_terminal', { id }).catch(() => {});
  },
  write(id, data) {
    invoke('write_terminal', { id, data }).catch(() => {});
  },
  resize(id, { cols, rows }) {
    invoke('resize_terminal', { id, cols, rows }).catch(() => {});
  },
  acceptsFiles: true,
};

interface RemoteOutput {
  id: string;
  base64: string;
  cols: number;
  rows: number;
}

/**
 * A PTY on another machine. Output arrives as `remote-terminal-output` events
 * addressed to this machine; keystrokes go back as `write_terminal` calls.
 *
 * **Writes are strictly ordered**: the next one is only sent once the previous
 * has been answered, and whatever is typed meanwhile travels together. Sending
 * them concurrently would let two keystrokes cross on the way, and "ls" could
 * arrive as "sl".
 */
export function remoteTransport(machine: RemoteMachine): TerminalTransport {
  const offs = new Map<string, () => void>();
  const queued = new Map<string, string>();
  const inFlight = new Set<string>();

  const flush = (id: string) => {
    const data = queued.get(id);
    if (!data || inFlight.has(id)) return;
    queued.delete(id);
    inFlight.add(id);
    machine
      .call('write_terminal', { id, data })
      .catch(() => {})
      .finally(() => {
        inFlight.delete(id);
        flush(id);
      });
  };

  return {
    async attach(id, onChunk) {
      offs.get(id)?.();
      offs.set(
        id,
        machine.on<RemoteOutput>('remote-terminal-output', (output) => {
          if (output.id === id) onChunk(output.base64, { cols: output.cols, rows: output.rows });
        }),
      );
      const attached = await machine.call<{ snapshot: string; cols: number; rows: number }>('remote_attach_terminal', { id });
      return { scrollback: attached.snapshot, size: { cols: attached.cols, rows: attached.rows } };
    },
    detach(id) {
      offs.get(id)?.();
      offs.delete(id);
      queued.delete(id);
      machine.call('remote_detach_terminal', { id }).catch(() => {});
    },
    write(id, data) {
      queued.set(id, (queued.get(id) ?? '') + data);
      flush(id);
    },
    resize: null,
    acceptsFiles: false,
  };
}
