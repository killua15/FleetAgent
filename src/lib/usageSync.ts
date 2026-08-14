import { invoke } from './invoke';
import { supabase } from './supabase';

/**
 * Your spend across every Mac signed in to this profile.
 *
 * Hangar is local-first and this is the one thing that leaves the machine, so
 * it is built to be defensible rather than convenient:
 *
 * - **Aggregates only.** A day, a project name, a subscription label, and
 *   counters. No prompts, no code, no file paths — the transcript never moves.
 * - **Opt-in.** Nothing is sent until the user turns it on; see
 *   `USAGE_SYNC_KEY`. Signing in is not consent to upload.
 * - **Yours only.** RLS on `usage_daily` scopes both reads and writes to
 *   `auth.uid()`, so this is enforced by the server and not by this file.
 *
 * It exists because one profile can run on two machines and each Hangar only
 * ever sees its own turns: the provider's quota reading is global and correct,
 * while the attribution by project is local, so the two never reconciled.
 */

/** Remembered per machine, because consenting on one Mac says nothing about
 * the other. */
export const USAGE_SYNC_KEY = 'hangar.usageSync';

export function usageSyncEnabled(): boolean {
  return window.localStorage.getItem(USAGE_SYNC_KEY) === 'on';
}

export function setUsageSyncEnabled(on: boolean): void {
  window.localStorage.setItem(USAGE_SYNC_KEY, on ? 'on' : 'off');
}

interface DailyUsage {
  machineId: string;
  machineName: string;
  day: string;
  projectName: string;
  providerEmail: string;
  accountLabel: string;
  provider: string;
  tokensIn: number;
  tokensOut: number;
  cacheRead: number;
  costUsd: number;
  turns: number;
}

/** How far back each push re-sends. */
const RESEND_DAYS = 14;

/**
 * Pushes this machine's recent days.
 *
 * Re-sends a window rather than only what changed: the primary key makes an
 * upsert idempotent, so re-sending is free, and a machine that was offline for
 * a week catches up without needing to have tracked what it missed.
 */
export async function pushUsage(): Promise<number> {
  if (!usageSyncEnabled()) return 0;
  const { data } = await supabase.auth.getUser();
  const userId = data.user?.id;
  // No session, nothing to attribute it to. Not an error: the app works signed
  // out, and this is the part that doesn't.
  if (!userId) return 0;

  const rows = await invoke<DailyUsage[]>('daily_usage_since', { days: RESEND_DAYS });
  // A machine with no id yet has never started the bridge; sending rows keyed
  // on an empty string would merge two Macs into one row.
  const sendable = rows.filter((r) => r.machineId !== '');
  if (sendable.length === 0) return 0;

  const { error } = await supabase.from('usage_daily').upsert(
    sendable.map((r) => ({
      user_id: userId,
      machine_id: r.machineId,
      machine_name: r.machineName,
      day: r.day,
      project_name: r.projectName,
      provider_email: r.providerEmail,
      account_label: r.accountLabel,
      provider: r.provider,
      tokens_in: r.tokensIn,
      tokens_out: r.tokensOut,
      cache_read: r.cacheRead,
      cost_usd: r.costUsd,
      turns: r.turns,
      updated_at: new Date().toISOString(),
    })),
    { onConflict: 'user_id,machine_id,day,project_name,provider_email,account_label' },
  );
  if (error) throw new Error(error.message);
  return sendable.length;
}

/** One machine's share of the profile's spend. */
export interface MachineTotal {
  machineId: string;
  machineName: string;
  costUsd: number;
  turns: number;
  /** True for the Mac this is running on, so the report can say "this one". */
  isThisMachine: boolean;
}

export interface ProfileUsage {
  costUsd: number;
  turns: number;
  machines: MachineTotal[];
  /** Days covered, so the number can be read against a period rather than
   * floating free. */
  from: string | null;
}

/**
 * The whole profile's spend, every machine included.
 *
 * Read from the server rather than merged locally: this machine cannot know
 * what the other one spent, which is the entire reason the table exists.
 */
export async function fetchProfileUsage(sinceDays: number): Promise<ProfileUsage | null> {
  const { data: auth } = await supabase.auth.getUser();
  if (!auth.user?.id) return null;

  const from = new Date(Date.now() - sinceDays * 86_400_000).toISOString().slice(0, 10);
  const { data, error } = await supabase
    .from('usage_daily')
    .select('machine_id, machine_name, cost_usd, turns, day')
    .gte('day', from);
  if (error) throw new Error(error.message);

  const thisMachine = await invoke<{ machineId: string | null }>('bridge_status')
    .then((s) => s.machineId)
    .catch(() => null);

  const byMachine = new Map<string, MachineTotal>();
  let costUsd = 0;
  let turns = 0;
  for (const row of data ?? []) {
    costUsd += row.cost_usd ?? 0;
    turns += row.turns ?? 0;
    const found = byMachine.get(row.machine_id);
    if (found) {
      found.costUsd += row.cost_usd ?? 0;
      found.turns += row.turns ?? 0;
    } else {
      byMachine.set(row.machine_id, {
        machineId: row.machine_id,
        machineName: row.machine_name || row.machine_id.slice(0, 8),
        costUsd: row.cost_usd ?? 0,
        turns: row.turns ?? 0,
        isThisMachine: row.machine_id === thisMachine,
      });
    }
  }

  return {
    costUsd,
    turns,
    machines: [...byMachine.values()].sort((a, b) => b.costUsd - a.costUsd),
    from,
  };
}
