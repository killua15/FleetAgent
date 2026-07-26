import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from 'react';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { invoke } from '../lib/invoke';
import { supabase, type Entitlement } from '../lib/supabase';

interface AuthContextValue {
  email: string | null;
  entitlement: Entitlement | null;
  /** True while the browser round trip is in flight. */
  signingIn: boolean;
  error: string | null;
  signIn: () => Promise<void>;
  signOut: () => Promise<void>;
}

const AuthContext = createContext<AuthContextValue | null>(null);

/**
 * Hangar's own account — identity and entitlements, nothing else.
 *
 * **The app works signed out.** It orchestrates CLIs the user already pays for;
 * demanding an account to open a terminal would be hostile. Signing in exists
 * for the things that genuinely need a server: backup/sync and the team plan.
 *
 * The refresh token lives in the OS keychain (`auth.rs`), not in localStorage,
 * so `persistSession` is off and this restores the session by hand on mount.
 */
export function AuthProvider({ children }: { children: ReactNode }) {
  const [email, setEmail] = useState<string | null>(null);
  const [entitlement, setEntitlement] = useState<Entitlement | null>(null);
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

  // Restore from the keychain on launch. A refresh token that's been revoked or
  // expired just fails here, which is indistinguishable from "not signed in" —
  // and that's the right outcome, not an error to show.
  useEffect(() => {
    (async () => {
      const refreshToken = await invoke<string | null>('load_refresh_token').catch(() => null);
      if (!refreshToken) return;
      const { data, error } = await supabase.auth.refreshSession({ refresh_token: refreshToken });
      if (error || !data.session) {
        await invoke('clear_refresh_token').catch(() => {});
        return;
      }
      setEmail(data.session.user.email ?? null);
      await invoke('store_refresh_token', { token: data.session.refresh_token }).catch(() => {});
      await loadEntitlement();
    })();
  }, [loadEntitlement]);

  // A rotated refresh token has to reach the keychain, or the next launch tries
  // to reuse one Supabase has already retired.
  useEffect(() => {
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

      const session = exchanged.data.session;
      setEmail(session.user.email ?? null);
      await invoke('store_refresh_token', { token: session.refresh_token });
      await loadEntitlement();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSigningIn(false);
    }
  }, [loadEntitlement]);

  const signOut = useCallback(async () => {
    await supabase.auth.signOut().catch(() => {});
    await invoke('clear_refresh_token').catch(() => {});
    setEmail(null);
    setEntitlement(null);
  }, []);

  return <AuthContext.Provider value={{ email, entitlement, signingIn, error, signIn, signOut }}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error('useAuth must be used within an AuthProvider');
  return ctx;
}
