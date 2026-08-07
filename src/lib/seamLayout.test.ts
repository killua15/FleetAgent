import { describe, expect, it } from 'vitest';
import { rowY, seamLayout, shortenPath } from './seamLayout';
import type { Bridge } from '../types/graph';

const bridge = (label: string, a: string, b: string): Bridge => ({
  label,
  ends: [
    { repo: 'g-fleet', nodeId: `g-fleet::${label}`, sourceFile: a },
    { repo: 'g-mobile', nodeId: `g-mobile::${label}`, sourceFile: b },
  ],
});

/** Real pairs from the Phase 0 spike, long paths included — those are the ones
 * that used to be cut off. */
const REAL: Bridge[] = [
  bridge('Envelope', 'src-tauri/src/bridge/protocol.rs', 'lib/bridge/envelope.dart'),
  bridge('PermissionRequest', 'src/types/session.ts', 'lib/domain/entities/permission_request.dart'),
  bridge('WorktreeChanges', 'src-tauri/src/commands/git_panel.rs', 'lib/domain/entities/worktree_changes.dart'),
  bridge('DiffLine', 'src/lib/diffLines.ts', 'lib/presentation/widgets/diff_view.dart'),
];

/** Same 9.5px mono the component draws with. */
const CHAR_WIDTH = 5.75;

describe('seamLayout', () => {
  /** The bug this exists to stop coming back. A viewBox clips permanently, so
   * a label that overflows it is gone — the container scrolls an already
   * cropped image and the text never comes back. */
  it('leaves room for the longest label on each side', () => {
    const layout = seamLayout(REAL)!;

    for (const file of layout.leftFiles) {
      const labelStart = layout.leftX - 10 - shortenPath(file).length * CHAR_WIDTH;
      expect(labelStart, `${file} starts left of the viewBox`).toBeGreaterThanOrEqual(0);
    }
    for (const file of layout.rightFiles) {
      const labelEnd = layout.rightX + 10 + shortenPath(file).length * CHAR_WIDTH;
      expect(labelEnd, `${file} runs past the right edge`).toBeLessThanOrEqual(layout.width);
    }
  });

  /** A longer path has to push the rail right, not get truncated against a
   * constant — which is exactly what the fixed-margin version did. */
  it('grows the gutter when a path gets longer', () => {
    const short = seamLayout([bridge('X', 'a.rs', 'b.dart')])!;
    const long = seamLayout([bridge('X', 'src-tauri/src/commands/an_extremely_long_module_name.rs', 'b.dart')])!;

    expect(long.leftX).toBeGreaterThan(short.leftX);
    expect(long.width).toBeGreaterThan(short.width);
  });

  /** Every row has to be inside the box too — the height grows with the rail,
   * and the container scrolls vertically rather than cropping. */
  it('is tall enough for every row on the longer rail', () => {
    const layout = seamLayout(REAL)!;
    const rows = Math.max(layout.leftFiles.length, layout.rightFiles.length);

    expect(rowY(layout, layout.leftFiles, layout.leftFiles[layout.leftFiles.length - 1])).toBeLessThan(layout.height);
    expect(layout.height).toBeGreaterThanOrEqual(layout.top + rows * layout.rowGap);
  });

  /** A file carrying several bridges appears once, which is what makes hubs
   * legible as hubs: in the measured pair, models.rs carries six. */
  it('lists a file once however many bridges land on it', () => {
    const layout = seamLayout([
      bridge('PullRequestInfo', 'src-tauri/src/models.rs', 'lib/domain/entities/pull_request.dart'),
      bridge('PrComment', 'src-tauri/src/models.rs', 'lib/domain/entities/pull_request.dart'),
      bridge('PrInboxEntry', 'src-tauri/src/models.rs', 'lib/domain/entities/pull_request.dart'),
    ])!;

    expect(layout.leftFiles).toEqual(['src-tauri/src/models.rs']);
    expect(layout.rightFiles).toEqual(['lib/domain/entities/pull_request.dart']);
  });

  /** One repo is not a seam. Returning null keeps the component from drawing
   * a rail against nothing. */
  it('has nothing to draw with fewer than two repos', () => {
    expect(seamLayout([])).toBeNull();
    expect(
      seamLayout([{ label: 'X', ends: [{ repo: 'only', nodeId: 'only::x', sourceFile: 'a.rs' }] }]),
    ).toBeNull();
  });

  it('shortens the noisy prefix and keeps the tail', () => {
    expect(shortenPath('lib/domain/entities/permission_request.dart')).toBe('permission_request.dart');
    expect(shortenPath('src-tauri/src/bridge/protocol.rs')).toBe('bridge/protocol.rs');
    expect(shortenPath('package.json')).toBe('package.json');
  });
});
