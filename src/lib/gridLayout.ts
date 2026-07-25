/** 1-2 panels fit in a single row; from 3 on up, always split into exactly 2
 * rows as evenly as possible (4 → 2+2, 6 → 3+3, 5 → 3+2, ...) rather than
 * growing into a single ever-wider row or an unbounded number of rows.
 *
 * Shared by the agent grid (`MainView`) and the terminal grid (`Terminals`)
 * so both feel like the same surface — a terminal pane and a session panel
 * laid out by different rules would read as two unrelated screens. */
export function autoGridColumns(panelCount: number): number {
  if (panelCount <= 0) return 1;
  const rows = panelCount <= 2 ? 1 : 2;
  return Math.ceil(panelCount / rows);
}
