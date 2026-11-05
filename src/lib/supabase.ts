import { createClient } from '@supabase/supabase-js';

/** The Supabase project this build talks to, set at build time from
 * `.env.local` (see `.env.example`). Both values are public by design — the
 * publishable key is what a browser client ships with, and everything it can
 * reach is guarded by row-level security, not by the key being secret — but
 * they name one particular backend, so they live with whoever builds the app
 * rather than in the repository. */
export const SUPABASE_URL: string = import.meta.env.VITE_SUPABASE_URL ?? '';
export const SUPABASE_PUBLISHABLE_KEY: string = import.meta.env.VITE_SUPABASE_PUBLISHABLE_KEY ?? '';

/** False for a build made without a Supabase project: the app can't sign
 * anyone in, and says so instead of failing on every request. */
export const supabaseConfigured = SUPABASE_URL.startsWith('https://') && SUPABASE_PUBLISHABLE_KEY.length > 0;

// `createClient` throws on an empty URL at import time, which would take the
// whole app down with it. Unconfigured, it gets an address nothing answers on,
// and every caller checks `supabaseConfigured` before using it.
export const supabase = createClient(supabaseConfigured ? SUPABASE_URL : 'http://127.0.0.1:9', supabaseConfigured ? SUPABASE_PUBLISHABLE_KEY : 'unconfigured', {
  auth: {
    // PKCE, because this is a desktop app: the code comes back to a loopback
    // port that any local process could reach, and the verifier — which never
    // leaves this webview — is what makes an intercepted code useless.
    flowType: 'pkce',
    // Hangar persists the refresh token in the OS keychain itself (see
    // `auth.rs`). supabase-js's default is localStorage, which anything running
    // in the webview can read.
    persistSession: false,
    autoRefreshToken: true,
  },
});

/** One user's plan, as the app cares about it. Read-only from the client: the
 * `subscriptions` table has no write policy, so only the Stripe webhook (service
 * role) can change it. */
export interface Entitlement {
  plan: 'free' | 'pro' | 'team';
  status: string;
  currentPeriodEnd: string | null;
}
