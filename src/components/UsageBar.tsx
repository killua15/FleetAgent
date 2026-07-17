/** Small colored progress bar + label — shared by AgentPanel's context/session
 * status line and MainView's per-account weekly-spend row, so both read as
 * the same visual language instead of two hand-rolled bars. */
export function UsageBar({
  pct,
  color,
  label,
  title,
  width = 90,
}: {
  pct: number;
  color: string;
  label: string;
  title?: string;
  width?: number;
}) {
  const clamped = Math.min(100, Math.max(0, pct));
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 6 }} title={title}>
      <div style={{ width, height: 4, borderRadius: 2, background: 'var(--panel2)', overflow: 'hidden', flex: 'none' }}>
        <div style={{ width: `${clamped}%`, height: '100%', borderRadius: 2, background: color }} />
      </div>
      <span style={{ font: '500 9.5px var(--font-mono)', color: 'var(--dim)', whiteSpace: 'nowrap' }}>{label}</span>
    </div>
  );
}
