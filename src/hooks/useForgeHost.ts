import { useEffect, useState } from 'react';
import { ALL_HOSTS } from '../lib/forgeHosts';

const STORAGE_KEY = 'hangar.forgeHost';

/**
 * Which forge instance the user is currently working against, shared by the
 * PR inbox and the git panel and remembered across restarts.
 *
 * A plain hook over `localStorage` rather than a context, unlike
 * `ThemeProvider`: the two screens are never mounted at once (`App.tsx`
 * switches on the route), so reading the stored value on mount already makes
 * the choice follow you from one tab to the other — a provider would add a
 * layer to the tree for no behaviour it doesn't already have.
 */
export function useForgeHost(): [string, (host: string) => void] {
  const [host, setHost] = useState<string>(() => window.localStorage.getItem(STORAGE_KEY) ?? ALL_HOSTS);

  useEffect(() => {
    window.localStorage.setItem(STORAGE_KEY, host);
  }, [host]);

  return [host, setHost];
}
