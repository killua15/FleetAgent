import { check, type Update } from '@tauri-apps/plugin-updater';
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
 * "Now" is the whole point. The `Update` this returns downloads with the same
 * headers it was checked with, and a Supabase access token lasts an hour — so
 * an update found at launch and installed after lunch downloaded with an
 * expired token, and Storage answered `400 Bad Request`. Calling this again
 * right before installing, instead of keeping the first `Update`, is the fix.
 * `getSession` refreshes a token that has expired.
 *
 * Signed out means no update: the channel can't tell who's asking.
 */
export async function checkForUpdate(): Promise<Update | null> {
  const { data } = await supabase.auth.getSession();
  const token = data.session?.access_token;
  if (!token) return null;
  return check({ headers: updateHeaders(token) });
}
