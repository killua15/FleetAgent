import { invoke } from '../lib/invoke';
import { supabase } from '../lib/supabase';

/** This machine, as Rust knows it. */
export interface MachineIdentity {
  machineId: string;
  machineName: string;
  publicKey: string;
  fingerprint: string;
  remoteAccess: boolean;
}

/** Another machine of the account, as `bridge_set_peers` returns it. */
export interface PeerView {
  id: string;
  name: string;
  /** `null` when that machine runs a Hangar too old to publish a key. */
  fingerprint: string | null;
  /** Whether it called in to this machine recently. */
  connected: boolean;
}

export interface MachineRow extends PeerView {
  lastSeenAt: string | null;
}

/**
 * Declares this machine as the signed-in account's, with its public key.
 *
 * **This is what authorizes the channel**: the `realtime.messages` policy only
 * lets the owner of `hangar:<id>` in, so without the row the private channel
 * refuses the subscription. The public key is what lets the account's other
 * machines derive a key with this one without a QR (`bridge/peers.rs`).
 *
 * Only called when a feature that needs it is in use (a phone, remote access,
 * or the Machines screen): signing in is not consent to list this machine.
 *
 * A failure is not swallowed: if the `machine_id` belongs to another account
 * the row can't be written, and starting anyway would give a channel that
 * never subscribes and an error much harder to read.
 */
export async function registerThisMachine(): Promise<MachineIdentity> {
  const identity = await invoke<MachineIdentity>('bridge_machine_identity');
  const { data } = await supabase.auth.getUser();
  const owner = data.user?.id;
  if (!owner) throw new Error('sin sesión no se puede registrar la máquina');

  const row = { id: identity.machineId, owner, name: identity.machineName, last_seen_at: new Date().toISOString() };
  let { error } = await supabase.from('machines').upsert({ ...row, public_key: identity.publicKey }, { onConflict: 'id' });
  // A project that has not run the `machines.public_key` migration yet
  // (PGRST204: column not in the schema cache). Registering without the key
  // keeps the phone's bridge working; only machine-to-machine waits for it.
  if (error?.code === 'PGRST204') {
    console.warn('[puente] machines.public_key no existe todavía: falta aplicar la migración');
    ({ error } = await supabase.from('machines').upsert(row, { onConflict: 'id' }));
  }
  if (error) throw new Error(`no se pudo registrar esta máquina: ${error.message}`);
  return identity;
}

/**
 * The account's other machines, also handed to Rust so it can derive a key
 * with each. RLS on `machines` already limits the rows to this account.
 */
export async function syncMachines(): Promise<MachineRow[]> {
  const { data, error } = await supabase.from('machines').select('id, name, public_key, last_seen_at');
  if (error?.code === '42703') throw new Error('falta aplicar la migración machines.public_key en Supabase');
  if (error) throw new Error(`no se pudieron leer las máquinas de la cuenta: ${error.message}`);

  const rows = (data ?? []) as Array<{ id: string; name: string; public_key: string | null; last_seen_at: string | null }>;
  const views = await invoke<PeerView[]>('bridge_set_peers', {
    peers: rows.map((row) => ({ id: row.id, name: row.name, publicKey: row.public_key })),
  });
  const lastSeen = new Map(rows.map((row) => [row.id, row.last_seen_at]));
  return views.map((view) => ({ ...view, lastSeenAt: lastSeen.get(view.id) ?? null }));
}
