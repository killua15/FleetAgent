/**
 * How many leading entries of a nav row fit in `available` pixels.
 *
 * Order is kept: entries leave from the end. When anything has to leave, room
 * is kept for the "More" button that will hold it. A separator left dangling
 * at the end of what fits goes too — a rule with nothing after it separates
 * nothing.
 *
 * `chrome` is what the row costs before any entry (padding, a leading button);
 * `gap` is the space after each entry.
 */
export function fitCount(
  widths: number[],
  kinds: Array<'item' | 'rule'>,
  available: number,
  chrome: number,
  gap: number,
  moreWidth: number,
): number {
  const total = (n: number) => chrome + widths.slice(0, n).reduce((sum, w) => sum + w + gap, 0);
  if (total(widths.length) <= available) return widths.length;
  let n = widths.length;
  while (n > 0 && total(n) + moreWidth + gap > available) n--;
  while (n > 0 && kinds[n - 1] === 'rule') n--;
  return n;
}
