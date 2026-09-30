/**
 * When a terminal will continue on its own, said the way a person reads a
 * clock: "today 15:03" when it's today, the weekday when it's later this week
 * (the weekly window), the date beyond that.
 */
export function formatResumeAt(iso: string, now: Date = new Date()): string {
  const at = new Date(iso);
  const time = `${String(at.getHours()).padStart(2, '0')}:${String(at.getMinutes()).padStart(2, '0')}`;
  const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const days = Math.round((startOfDay(at) - startOfDay(now)) / 86_400_000);
  if (days <= 0) return `today ${time}`;
  if (days === 1) return `tomorrow ${time}`;
  if (days < 7) return `${at.toLocaleDateString('en-US', { weekday: 'short' })} ${time}`;
  return `${at.toLocaleDateString('en-US', { month: 'short', day: 'numeric' })} ${time}`;
}
