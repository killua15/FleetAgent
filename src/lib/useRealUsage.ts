import { useCallback, useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { RealUsageStats } from '../types/session';

/** Fetches Claude Code's real `/usage` percentages for a subscription
 * account in the background — never blocks the caller, which should keep
 * showing its own token-based estimate until (if ever) this resolves with a
 * value, and fall back to it again for any field that comes back `null`.
 * The backend caches per-account for a few minutes; `refresh()` forces a
 * live re-scrape (slow — spawns a real interactive `claude` process, so this
 * only ever runs once automatically per mount/account, never on a poll). */
export function useRealUsage(accountId: string | undefined, enabled: boolean) {
  const [stats, setStats] = useState<RealUsageStats | null>(null);
  const [loading, setLoading] = useState(false);

  const fetchUsage = useCallback(
    (force: boolean) => {
      if (!accountId) return;
      setLoading(true);
      invoke<RealUsageStats>('fetch_real_usage', { accountId, force })
        .then(setStats)
        .catch(() => {})
        .finally(() => setLoading(false));
    },
    [accountId],
  );

  useEffect(() => {
    if (!enabled || !accountId) return;
    fetchUsage(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled, accountId]);

  return { stats, loading, refresh: () => fetchUsage(true) };
}
