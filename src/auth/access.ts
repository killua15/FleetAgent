/**
 * Whether this launch gets to use Hangar, and if not, why not.
 *
 * The decision lives here rather than inside the provider because it is the one
 * part of the gate that is pure: given what the server said (or failed to say)
 * and what the keychain holds, the answer is a lookup, not a side effect. That
 * makes it testable without a session, a network, or a webview.
 */

/** `checking` is the state on mount, before the refresh has come back.
 * `unconfigured`: this build has no Supabase project to sign in against. */
export type Access = 'checking' | 'signed-out' | 'pending' | 'ready' | 'grace-expired' | 'unconfigured';

export interface AccessInput {
  /** A live session — the refresh succeeded, or one was already in memory. */
  hasSession: boolean;
  /** `profiles.approved` for this user. Unknown while offline. */
  approved: boolean | null;
  /** A refresh token sits in the keychain, whether or not it could be checked. */
  hasStoredToken: boolean;
  /**
   * Did Supabase answer at all? A refused token and an unreachable server both
   * come back as an error from `refreshSession`, and treating them the same is
   * what turns a flaky connection into a lockout.
   */
  serverReachable: boolean;
  /** `within_grace` as answered by Rust — the stamp lives in `app_settings`. */
  graceIsLive: boolean;
}

export function resolveAccess(input: AccessInput): Access {
  const { hasSession, approved, hasStoredToken, serverReachable, graceIsLive } = input;

  // Offline is judged first: with no answer from the server, `approved` is
  // stale at best and absent at worst, so the only question worth asking is
  // whether this machine checked in recently enough.
  if (!serverReachable) {
    if (!hasStoredToken) return 'signed-out';
    return graceIsLive ? 'ready' : 'grace-expired';
  }

  if (!hasSession) return 'signed-out';
  return approved ? 'ready' : 'pending';
}

/** Screens the gate shows instead of the app. `ready` isn't one of them. */
export function isBlocked(access: Access): boolean {
  return access !== 'ready';
}
