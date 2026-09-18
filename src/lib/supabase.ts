import { createClient } from '@supabase/supabase-js';

/** Hangar's Supabase project. Both values are public by design — the
 * publishable key is what a browser client ships with, and everything it can
 * reach is guarded by row-level security, not by the key being secret. */
export const SUPABASE_URL = 'https://yqrvofogmyqdqufyxlfk.supabase.co';
export const SUPABASE_PUBLISHABLE_KEY = 'sb_publishable_CTwAWQ70NlE0dEzTpv2zhQ_M0k9SbiQ';

export const supabase = createClient(SUPABASE_URL, SUPABASE_PUBLISHABLE_KEY, {
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
