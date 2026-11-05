import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import { getVersion } from '@tauri-apps/api/app';
import { openUrl } from '@tauri-apps/plugin-opener';
import { invoke } from '../lib/invoke';
import { supabase, supabaseConfigured, type Entitlement } from '../lib/supabase';
import { resolveAccess, type Access } from './access';

interface AuthContextValue {
  email: string | null;
  /** The name the identity provider knows this person by, when it sends one.
   * Google does; a future provider might not, which is why the email is what
   * the account section falls back to rather than the other way round. */
  displayName: string | null;
  avatarUrl: string | null;
  entitlement: Entitlement | null;
  /** Whether this launch may use the app — see `access.ts`. */
  access: Access;
  /** True while the browser round trip is in flight. */
  signingIn: boolean;
  error: string | null;
  signIn: () => Promise<void>;
  signOut: () => Promise<void>;
  /** Asks the server again — for the "pending approval" screen's retry. */
  recheck: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

/**
 * Hangar's own account — identity, entitlements, and the gate.
 *
 * **The app requires an account.** Not to hold the CLIs hostage — they're the
 * user's, and Hangar only drives them — but because there was otherwise no way
 * to know who is running this, and no way to stop someone who shouldn't be.
 * `profiles.approved` is the switch, and it lives server-side precisely so the
 * client can't argue with it.
 *
 * The refresh token lives in the OS keychain (`auth.rs`), not in localStorage,
 * so `persistSession` is off and this restores the session by hand on mount.
 */
export function AuthProvider({ children }: { children: ReactNode }) {
  const [email, setEmail] = useState<string | null>(null);
  const [displayName, setDisplayName] = useState<string | null>(null);
  const [avatarUrl, setAvatarUrl] = useState<string | null>(null);
  const [entitlement, setEntitlement] = useState<Entitlement | null>(null);
  const [access, setAccess] = useState<Access>(supabaseConfigured ? 'checking' : 'unconfigured');
  const [signingIn, setSigningIn] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadEntitlement = useCallback(async () => {
    const { data, error } = await supabase.from('subscriptions').select('plan, status, current_period_end').maybeSingle();
    if (error || !data) {
      setEntitlement(null);
      return;
    }
    setEntitlement({ plan: data.plan, status: data.status, currentPeriodEnd: data.current_period_end });
  }, []);

  /**
   * Records that this person, on this machine, opened this version.
   *
   * Best-effort on purpose: a failed write here is a gap in the usage record,
   * not a reason to refuse someone the server just approved.
   */
  const recordPresence = useCallback(async (userId: string) => {
    try {
      const [version, status] = await Promise.all([
        getVersion().catch(() => null),
        invoke<{ machineName: string }>('bridge_status').catch(() => null),
      ]);
      await supabase
        .from('profiles')
        .update({
          last_seen_at: new Date().toISOString(),
          app_version: version,
          last_machine: status?.machineName ?? null,
          updated_at: new Date().toISOString(),
        })
        .eq('id', userId);
    } catch {
      /* ignored — see above */
    }
  }, []);

  /**
   * Asks Supabase whether this session is still good and this account is still
   * allowed, and settles `access` accordingly.
   *
   * The distinction that matters: `refreshSession` fails both when the token
   * has been revoked and when there is no network, and the two must not be
   * treated alike. A revoked token is an eviction — clear it and ask for a new
   * sign-in. An unreachable server is a plane, and the answer there is the
   * grace window, not a lockout. supabase-js marks the latter as retryable and
   * gives it no HTTP status.
   */
  const check = useCallback(async () => {
    const refreshToken = await invoke<string | null>('load_refresh_token').catch(() => null);
    const graceIsLive = await invoke<boolean>('auth_grace_is_live').catch(() => false);

    if (!refreshToken) {
      setAccess(resolveAccess({ hasSession: false, approved: null, hasStoredToken: false, serverReachable: true, graceIsLive: false }));
      return;
    }

    const { data, error } = await supabase.auth.refreshSession({ refresh_token: refreshToken });

    if (error && isOffline(error)) {
      setAccess(resolveAccess({ hasSession: false, approved: null, hasStoredToken: true, serverReachable: false, graceIsLive }));
      return;
    }

    if (error || !data.session) {
      // The server answered and said no. Keeping the token would mean retrying
      // a credential Supabase has already retired, every launch, forever.
      await invoke('clear_refresh_token').catch(() => {});
      setEmail(null);
      setDisplayName(null);
      setAvatarUrl(null);
      setEntitlement(null);
      setAccess(resolveAccess({ hasSession: false, approved: null, hasStoredToken: false, serverReachable: true, graceIsLive }));
      return;
    }

    const session = data.session;
    setEmail(session.user.email ?? null);
    // Whatever the provider chose to send. Google fills `full_name` and
    // `avatar_url`; the `name`/`picture` pair is what the raw OIDC claim uses,
    // so both spellings are read rather than betting on one.
    const metadata = session.user.user_metadata ?? {};
    setDisplayName(typeof metadata.full_name === 'string' ? metadata.full_name : typeof metadata.name === 'string' ? metadata.name : null);
    setAvatarUrl(typeof metadata.avatar_url === 'string' ? metadata.avatar_url : typeof metadata.picture === 'string' ? metadata.picture : null);
    await invoke('store_refresh_token', { token: session.refresh_token }).catch(() => {});

    const { data: profile } = await supabase.from('profiles').select('approved').eq('id', session.user.id).maybeSingle();
    const approved = profile?.approved ?? false;

    if (approved) {
      // Only a real answer from the server refreshes the stamp. Stamping on
      // every launch would let a machine coast offline indefinitely, each
      // launch buying another week.
      await invoke('record_auth_check').catch(() => {});
      await recordPresence(session.user.id);
      await loadEntitlement();
    }

    setAccess(resolveAccess({ hasSession: true, approved, hasStoredToken: true, serverReachable: true, graceIsLive }));
  }, [loadEntitlement, recordPresence]);

  useEffect(() => {
    // Without a project there is nothing to ask, and every request would be a
    // timeout against an address nothing listens on.
    if (supabaseConfigured) void check();
  }, [check]);

  // A rotated refresh token has to reach the keychain, or the next launch tries
  // to reuse one Supabase has already retired.
  useEffect(() => {
    if (!supabaseConfigured) return;
    const { data } = supabase.auth.onAuthStateChange((_event, session) => {
      if (session?.refresh_token) invoke('store_refresh_token', { token: session.refresh_token }).catch(() => {});
    });
    return () => data.subscription.unsubscribe();
  }, []);

  const signIn = useCallback(async () => {
    setSigningIn(true);
    setError(null);
    try {
      // The listener has to be up before the browser can land on it.
      const redirectTo = await invoke<string>('start_oauth_callback');

      const done = new Promise<string>((resolve, reject) => {
        const unlisten = listen<{ code?: string; error?: string }>('oauth-callback', (event) => {
          unlisten.then((f) => f());
          if (event.payload.code) resolve(event.payload.code);
          else reject(new Error(event.payload.error ?? 'sign-in failed'));
        });
      });

      const { data, error } = await supabase.auth.signInWithOAuth({
        provider: 'google',
        options: { redirectTo, skipBrowserRedirect: true },
      });
      if (error) throw error;
      if (!data.url) throw new Error('Supabase returned no sign-in URL');

      // The system browser, not the webview: this is where the user's Google
      // session already lives, and where they can see the real URL bar.
      await openUrl(data.url);

      const code = await done;
      const exchanged = await supabase.auth.exchangeCodeForSession(code);
      if (exchanged.error) throw exchanged.error;

      await invoke('store_refresh_token', { token: exchanged.data.session.refresh_token });
      // One path decides access, whether the session came from the keychain or
      // from the browser just now.
      await check();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSigningIn(false);
    }
  }, [check]);

  const signOut = useCallback(async () => {
    await supabase.auth.signOut().catch(() => {});
    await invoke('clear_refresh_token').catch(() => {});
    setEmail(null);
    setDisplayName(null);
    setAvatarUrl(null);
    setEntitlement(null);
    setAccess('signed-out');
  }, []);

  const recheck = useCallback(async () => {
    if (!supabaseConfigured) return;
    setAccess('checking');
    setError(null);
    await check();
  }, [check]);

  return (
    <AuthContext.Provider value={{ email, displayName, avatarUrl, entitlement, access, signingIn, error, signIn, signOut, recheck }}>
      {children}
    </AuthContext.Provider>
  );
}

/**
 * A failure to reach Supabase at all, as opposed to Supabase saying no.
 *
 * supabase-js wraps the first in `AuthRetryableFetchError`, which carries no
 * HTTP status because no HTTP response ever arrived. The name check is the
 * documented signal; the missing status is the belt to its braces, since a
 * bundler that mangles class names would otherwise silently turn every offline
 * launch into a sign-out.
 */
function isOffline(error: { name?: string; status?: number }): boolean {
  return error.name === 'AuthRetryableFetchError' || !error.status;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth must be used within an AuthProvider');
  return ctx;
}
