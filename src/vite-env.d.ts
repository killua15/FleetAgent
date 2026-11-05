/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** The Supabase project URL, e.g. `https://<ref>.supabase.co`. From `.env.local`. */
  readonly VITE_SUPABASE_URL?: string;
  /** The project's publishable (`sb_publishable_…`) key. From `.env.local`. */
  readonly VITE_SUPABASE_PUBLISHABLE_KEY?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
