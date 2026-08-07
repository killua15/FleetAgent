import { useMemo, useState } from 'react';
import { rowY, seamLayout, shortenPath } from '../lib/seamLayout';
import type { Bridge } from '../types/graph';
import styles from './SeamMap.module.css';

/**
 * The seam between two repos: two rails, one per repo, one curve per bridge.
 *
 * Not a force-directed graph, and not because that would be harder. A hairball
 * of 4 480 nodes looks good in a screenshot and answers no question a group
 * actually has — and graphify's own HTML already draws one. This answers the
 * only question a group has: *where do these two repos touch, and which file
 * sits on each side?* (`GRAPHIFY_TRD.md` §5.8.)
 *
 * Drawn by Hangar rather than embedding graphify's `graph.html`, which pulls
 * vis-network from `unpkg.com` — blank under the app's CSP, blank offline, and
 * in a single dark theme that is not ours.
 *
 * A file appears once however many bridges land on it, which is what makes
 * hubs legible as hubs: in the measured pair, `models.rs` and
 * `pull_request.dart` carry six each, and those are where a careless edit
 * breaks the other side.
 */
export function SeamMap({ bridges }: { bridges: Bridge[] }) {
  const [focus, setFocus] = useState<{ kind: 'bridge'; index: number } | { kind: 'file'; file: string } | null>(null);

  // The geometry lives in `src/lib/seamLayout.ts`, with tests: it is a pure
  // decision, and it is where the clipping bug was.
  const layout = useMemo(() => seamLayout(bridges), [bridges]);
  if (!layout) return null;

  const { left, right, leftFiles, rightFiles, leftX: LX, rightX: RX, width, height } = layout;
  const y = (files: string[], f: string) => rowY(layout, files, f);

  const endIn = (b: Bridge, repo: string) => b.ends.find((e) => e.repo === repo);
  const touches = (b: Bridge, file: string) => b.ends.some((e) => e.sourceFile === file);

  const isOn = (i: number, b: Bridge) =>
    focus?.kind === 'bridge' ? focus.index === i : focus?.kind === 'file' ? touches(b, focus.file) : false;
  const fileOn = (file: string) =>
    focus?.kind === 'file' ? focus.file === file : focus?.kind === 'bridge' ? touches(bridges[focus.index], file) : false;

  const readout =
    focus?.kind === 'bridge'
      ? `${bridges[focus.index].label} — ${bridges[focus.index].ends.map((e) => e.sourceFile).join('  ↔  ')}`
      : focus?.kind === 'file'
        ? `${focus.file} — ${bridges.filter((b) => touches(b, focus.file)).length} tipo(s) compartidos`
        : `${bridges.length} puente${bridges.length === 1 ? '' : 's'} · 0 aristas AST cruzan`;

  return (
    <div className={styles.wrap}>
      <div className={styles.scroll}>
        {/* Explicit pixel size, not `width: 100%`. Scaling to the column would
            shrink 9.5px labels to unreadable in a narrow panel, and it is also
            what stopped the container from ever overflowing — so the scrollbar
            never appeared and long rails simply vanished off the bottom. */}
        <svg
          width={width}
          height={height}
          viewBox={`0 0 ${width} ${height}`}
          className={styles.svg}
          role="img"
          aria-label={`Costura entre ${left} y ${right}`}
        >
          <text x={LX} y={16} textAnchor="end" className={styles.railHead} fill="var(--acc)">
            {left}
          </text>
          <text x={RX} y={16} className={styles.railHead} fill="var(--teal)">
            {right}
          </text>

          {bridges.map((b, i) => {
            const a = endIn(b, left);
            const z = endIn(b, right);
            if (!a || !z) return null;
            const y1 = y(leftFiles, a.sourceFile);
            const y2 = y(rightFiles, z.sourceFile);
            const mid = (LX + RX) / 2;
            const on = isOn(i, b);
            return (
              <path
                key={b.label}
                d={`M ${LX + 8} ${y1} C ${mid} ${y1}, ${mid} ${y2}, ${RX - 8} ${y2}`}
                className={`${styles.bridge} ${on ? styles.on : focus ? styles.dim : ''}`}
                stroke={i % 2 ? 'var(--teal)' : 'var(--acc)'}
                onMouseEnter={() => setFocus({ kind: 'bridge', index: i })}
                onMouseLeave={() => setFocus(null)}
              />
            );
          })}

          {[
            { files: leftFiles, x: LX, anchor: 'end' as const, color: 'var(--acc)' },
            { files: rightFiles, x: RX, anchor: 'start' as const, color: 'var(--teal)' },
          ].map(({ files, x, anchor, color }) =>
            files.map((f) => (
              <g
                key={`${x}-${f}`}
                className={styles.fileNode}
                tabIndex={0}
                onMouseEnter={() => setFocus({ kind: 'file', file: f })}
                onMouseLeave={() => setFocus(null)}
                onFocus={() => setFocus({ kind: 'file', file: f })}
                onBlur={() => setFocus(null)}
              >
                <title>{f}</title>
                <circle cx={x} cy={y(files, f)} r={3.5} fill={color} />
                <text
                  x={x + (anchor === 'end' ? -10 : 10)}
                  y={y(files, f) + 3.5}
                  textAnchor={anchor}
                  className={`${styles.fileLabel} ${fileOn(f) ? styles.labelOn : ''}`}
                >
                  {shortenPath(f)}
                </text>
              </g>
            )),
          )}
        </svg>
      </div>
      <div className={styles.readout}>{readout}</div>
    </div>
  );
}
