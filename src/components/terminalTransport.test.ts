import { describe, expect, it } from 'vitest';
import type { RemoteMachine } from '../bridge/remote';
import { remoteTransport } from './terminalTransport';

/** A machine whose `write_terminal` answers only when the test says so. */
function fakeMachine() {
  const writes: string[] = [];
  const answers: Array<() => void> = [];
  const listeners = new Map<string, Set<(data: unknown) => void>>();
  const machine = {
    call(name: string, args: Record<string, unknown>) {
      if (name === 'write_terminal') {
        writes.push(String(args.data));
        return new Promise<void>((resolve) => answers.push(resolve));
      }
      if (name === 'remote_attach_terminal') return Promise.resolve({ snapshot: '', cols: 100, rows: 30 });
      return Promise.resolve(undefined);
    },
    on(name: string, listener: (data: unknown) => void) {
      const set = listeners.get(name) ?? new Set();
      set.add(listener);
      listeners.set(name, set);
      return () => set.delete(listener);
    },
    emit(name: string, data: unknown) {
      for (const listener of listeners.get(name) ?? []) listener(data);
    },
  };
  return { machine, writes, answers };
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

describe('remote terminal transport', () => {
  // Two keystrokes in flight at once could cross on the way: "ls" as "sl".
  it('sends one write at a time and batches what was typed meanwhile', async () => {
    const { machine, writes, answers } = fakeMachine();
    const transport = remoteTransport(machine as unknown as RemoteMachine);

    transport.write('t', 'l');
    transport.write('t', 's');
    transport.write('t', '\r');
    expect(writes).toEqual(['l']);

    answers.shift()!();
    await settle();
    expect(writes).toEqual(['l', 's\r']);

    answers.shift()!();
    await settle();
    expect(writes).toEqual(['l', 's\r']);
  });

  it('only passes on output for its own terminal, with the remote size', async () => {
    const { machine } = fakeMachine();
    const transport = remoteTransport(machine as unknown as RemoteMachine);
    const seen: Array<[string, unknown]> = [];

    const attached = await transport.attach('t', (base64, size) => seen.push([base64, size]));
    expect(attached.size).toEqual({ cols: 100, rows: 30 });

    machine.emit('remote-terminal-output', { id: 'other', base64: 'AA==', cols: 80, rows: 24 });
    machine.emit('remote-terminal-output', { id: 't', base64: 'AQ==', cols: 100, rows: 30 });
    expect(seen).toEqual([['AQ==', { cols: 100, rows: 30 }]]);

    transport.detach('t');
    machine.emit('remote-terminal-output', { id: 't', base64: 'Ag==', cols: 100, rows: 30 });
    expect(seen).toHaveLength(1);
  });

  it('never resizes the other machine and refuses dropped files', () => {
    const { machine } = fakeMachine();
    const transport = remoteTransport(machine as unknown as RemoteMachine);
    expect(transport.resize).toBeNull();
    expect(transport.acceptsFiles).toBe(false);
  });
});
