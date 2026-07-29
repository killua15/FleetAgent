/** Number formatting for the usage modal.
 *
 * Separate from `tokenBudget`'s `formatTokenCount` on purpose, and they
 * deliberately differ: that one labels a budget bar (`28.2k`), this one fills a
 * column of figures compared against each other (`28K`). Merging them would
 * force one of the two screens to read wrong. */

/** Token counts, for a table where the magnitude is what matters. */
export function compactTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${Math.round(n / 1000)}K`;
  return String(n);
}

/** Money, with enough decimals to stay non-zero.
 *
 * A single turn costs fractions of a cent, and `$0.00` on every row would make
 * the whole table look broken — so anything under a dollar keeps three
 * decimals. Above a dollar, cents are what people read. */
export function money(n: number): string {
  return n >= 1 ? `$${n.toFixed(2)}` : `$${n.toFixed(3)}`;
}
