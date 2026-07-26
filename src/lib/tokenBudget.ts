/** Approximate context-window sizes per provider — Hangar has no API to ask
 * "what's my model's real limit," so these are reasonable, documented
 * estimates, not exact figures. */
const CONTEXT_WINDOW_BY_PROVIDER: Record<string, number> = {
  claude: 200_000,
  codex: 128_000,
  gemini: 1_000_000,
};

const DEFAULT_CONTEXT_WINDOW = 200_000;

export function contextWindowFor(provider: string): number {
  return CONTEXT_WINDOW_BY_PROVIDER[provider] ?? DEFAULT_CONTEXT_WINDOW;
}

/** Subscription plans have no per-token price, so there's no real dollar
 * signal to show — but Hangar also has no API access to Anthropic's actual
 * 5h/7d rate-limit quota to show a real percentage either. These budgets are
 * a rough, clearly-estimated stand-in ("a generous long session" / "a heavy
 * week"), scaled off the same per-provider context window so they land in a
 * sane ballpark across providers rather than one flat guess. */
export function sessionTokenBudgetFor(provider: string): number {
  return contextWindowFor(provider) * 5;
}

export function weeklyTokenBudgetFor(provider: string): number {
  return contextWindowFor(provider) * 50;
}

export type ContextSeverity = 'ok' | 'warn' | 'danger';

/** Mirrors Claude Code's own status-line guidance: comfortable below ~70%,
 * worth watching up to ~90%, past that a compact/new-session nudge. */
export function contextSeverity(pct: number): ContextSeverity {
  if (pct >= 90) return 'danger';
  if (pct >= 70) return 'warn';
  return 'ok';
}

export function contextSeverityColor(severity: ContextSeverity): string {
  if (severity === 'danger') return 'var(--red)';
  if (severity === 'warn') return 'var(--amber)';
  return 'var(--grn)';
}

export function formatTokenCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}m`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function formatUsd(n: number): string {
  return `$${n.toFixed(2)}`;
}
