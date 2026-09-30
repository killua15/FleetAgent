import { useSyncExternalStore } from 'react';
import { check, type Update } from '@tauri-apps/plugin-updater';
import { invoke } from './invoke';
import { supabase, SUPABASE_PUBLISHABLE_KEY } from './supabase';

/**
 * The headers every update request carries: the tester's own Supabase session,
 * which the `updates` Edge Function and the Storage bucket both gate on.
 *
 * No `Accept` on purpose: the plugin sets `application/json` for the manifest
 * and `application/octet-stream` for the download, and pinning one would break
 * the other.
 */
export function updateHeaders(token: string): Record<string, string> {
  return { Authorization: `Bearer ${token}`, apikey: SUPABASE_PUBLISHABLE_KEY };
}

/**
 * Asks the update channel for a newer build, with a session token fetched now.
 *
 * "Now" matters: the `Update` this returns downloads with the headers it was
 * checked with, and a Supabase access token lasts an hour. `getSession`
 * refreshes one that has expired. Signed out means no update: the channel
 * can't tell who's asking.
 */
export async function checkForUpdate(): Promise<Update | null> {
  const { data } = await supabase.auth.getSession();
  const token = data.session?.access_token;
  if (!token) return null;
  return check({ headers: updateHeaders(token) });
}

/** Where self-updating stands, for the banner and for Settings. */
export type UpdateStatus =
  | { phase: 'idle' }
  | { phase: 'checking' }
  | { phase: 'installing'; version: string }
  | { phase: 'ready'; version: string }
  | { phase: 'upToDate' }
  | { phase: 'failed'; version: string | null; error: string };

let status: UpdateStatus = { phase: 'idle' };
let running: Promise<void> | null = null;
const listeners = new Set<() => void>();

function set(next: UpdateStatus) {
  status = next;
  listeners.forEach((l) => l());
}

export function getUpdateStatus(): UpdateStatus {
  return status;
}

export function subscribeToUpdates(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function useUpdateStatus(): UpdateStatus {
  return useSyncExternalStore(subscribeToUpdates, getUpdateStatus);
}

/**
 * Checks, and when there's a newer build, downloads and installs it straight
 * away — the user's only step is restarting.
 *
 * Straight away is what makes it work: the install uses the same fresh token as
 * the check, seconds later, so it can't expire in between (it used to, when the
 * install waited for a click). On macOS the install replaces the app on disk
 * and the running copy carries on until the restart.
 *
 * One run at a time, and none once a build is waiting for a restart: the timer,
 * window focus and the Settings button all call this, and a second install of
 * the same version would only redo work.
 */
export function runUpdateCheck(): Promise<void> {
  if (running) return running;
  if (status.phase === 'ready') return Promise.resolve();
  running = (async () => {
    set({ phase: 'checking' });
    let version: string | null = null;
    try {
      const update = await checkForUpdate();
      if (!update) {
        set({ phase: 'upToDate' });
        return;
      }
      version = update.version;
      set({ phase: 'installing', version });
      await update.downloadAndInstall();
      set({ phase: 'ready', version });
      invoke('notify_update_available', { version }).catch(() => {});
    } catch (e) {
      const error = String(e);
      set({ phase: 'failed', version, error });
      // The banner is gone once dismissed; the log keeps it for diagnosis.
      invoke('log_update_error', { version, error }).catch(() => {});
    } finally {
      running = null;
    }
  })();
  return running;
}

/** Test seam: back to a fresh app's state. */
export function resetUpdateStatusForTests() {
  status = { phase: 'idle' };
  running = null;
  listeners.clear();
}
