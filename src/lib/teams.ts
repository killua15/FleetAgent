import { supabase } from './supabase';

/** One account in a shared team pool. Note what isn't here: no config dir, no
 * token. The pool records that an account exists and who is using it right now;
 * credentials stay in each machine's keychain and Hangar never sees them. */
export interface TeamAccount {
  id: string;
  teamId: string;
  label: string;
  provider: string;
}

export interface AccountLease {
  teamAccountId: string;
  userId: string;
  acquiredAt: string;
  expiresAt: string;
  note: string | null;
}

/** How long a lease lasts before it expires on its own. Short on purpose: a
 * client that crashes holding one would otherwise block that account forever,
 * and the team would be back to asking in Slack — except now the tool is lying
 * to them. It's renewed by heartbeat while a terminal is actually open. */
export const LEASE_MINUTES = 30;

/** Renew well before expiry, so one missed tick doesn't drop a lease that's
 * still in use. */
export const HEARTBEAT_MS = (LEASE_MINUTES / 3) * 60 * 1000;

export async function listPool(teamId: string): Promise<TeamAccount[]> {
  const { data, error } = await supabase.from('team_accounts').select('id, team_id, label, provider').eq('team_id', teamId);
  if (error || !data) return [];
  return data.map((row) => ({ id: row.id, teamId: row.team_id, label: row.label, provider: row.provider }));
}

/** Who holds what, right now. Expired leases are filtered server-side by the
 * same clock that issues them — comparing against the client's clock would let
 * a skewed machine see a free account that isn't. */
export async function listLiveLeases(teamId: string): Promise<AccountLease[]> {
  const { data, error } = await supabase
    .from('account_leases')
    .select('team_account_id, user_id, acquired_at, expires_at, note, team_accounts!inner(team_id)')
    .is('released_at', null)
    .gt('expires_at', new Date().toISOString())
    .eq('team_accounts.team_id', teamId);
  if (error || !data) return [];
  return data.map((row) => ({
    teamAccountId: row.team_account_id,
    userId: row.user_id,
    acquiredAt: row.acquired_at,
    expiresAt: row.expires_at,
    note: row.note,
  }));
}

/**
 * Takes (or renews) the lease on a pooled account.
 *
 * Goes through a database function rather than an insert, because two clients
 * asking "is it free?" and then both writing is exactly the collision this
 * feature exists to prevent. The check and the write happen under one row lock,
 * server-side — and there is no insert policy on the table, so this is the only
 * way in.
 *
 * Acquiring and renewing are the same call: if the lease is already yours it
 * extends, which means the client needs one code path instead of two.
 */
export async function acquireLease(teamAccountId: string, note?: string): Promise<AccountLease | { heldBy: string; until: string }> {
  const { data, error } = await supabase.rpc('acquire_account_lease', {
    account: teamAccountId,
    minutes: LEASE_MINUTES,
    note: note ?? null,
  });
  if (error) {
    // "held by someone else until X" is the expected answer, not a failure —
    // it's the information the user asked for by clicking.
    const match = /held by someone else until (.+)$/.exec(error.message);
    if (match) return { heldBy: 'someone else', until: match[1] };
    throw new Error(error.message);
  }
  return {
    teamAccountId: data.team_account_id,
    userId: data.user_id,
    acquiredAt: data.acquired_at,
    expiresAt: data.expires_at,
    note: data.note,
  };
}

export async function releaseLease(teamAccountId: string): Promise<void> {
  await supabase.rpc('release_account_lease', { account: teamAccountId });
}
