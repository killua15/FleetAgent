/**
 * The top bar's icons, drawn rather than borrowed from Unicode.
 *
 * What was there before was nine unrelated glyphs — `⌗` for terminals, `⌸` for
 * files, `◐` for accounts — each from a different part of Unicode, with
 * different stroke weights and optical sizes, and none of them drawing the
 * thing it named. Plus one emoji, which the OS renders in its own font at its
 * own size and breaks the row.
 *
 * These are all the same: one 16×16 box, 1.5px stroke, `currentColor`, no
 * fills. That consistency is the entire point — an icon row is read as a row,
 * and one glyph in the wrong weight makes the whole set look accidental.
 */
const base = {
  width: 16,
  height: 16,
  viewBox: '0 0 16 16',
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.5,
  strokeLinecap: 'round',
  strokeLinejoin: 'round',
} as const;

export function HomeIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <path d="M2.5 6.8 8 2.5l5.5 4.3V13a.5.5 0 0 1-.5.5H3a.5.5 0 0 1-.5-.5z" />
    </svg>
  );
}

/** A prompt in a window — the thing terminals actually look like. */
export function TerminalIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" />
      <path d="M4.5 6.5 6.5 8l-2 1.5M8.5 10h3" />
    </svg>
  );
}

/** A checklist: what a board of tasks is. */
export function TasksIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <path d="M2 4.5 3 5.5l2-2M2 11.5l1 1 2-2M7.5 4.5h7M7.5 11.5h7" />
    </svg>
  );
}

/**
 * A compass needle: where to go, not what to do.
 *
 * Deliberately not a checklist — `TasksIcon` is already a checklist, and this
 * screen is about orientation in an app with 28 of them, which is a different
 * idea and deserves a different shape.
 */
export function CompassIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <circle cx="8" cy="8" r="6" />
      <path d="M10.2 5.8 6.9 6.9 5.8 10.2l3.3-1.1z" />
    </svg>
  );
}

/** Two branches meeting — a pull request, not a speech bubble. */
export function PullRequestIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <circle cx="4" cy="4" r="1.8" />
      <circle cx="4" cy="12" r="1.8" />
      <circle cx="12" cy="12" r="1.8" />
      <path d="M4 5.8v4.4M12 10.2V7a2 2 0 0 0-2-2H7.5M9 3.2 7 5l2 1.8" />
    </svg>
  );
}

/** The branch symbol everyone already knows from git. */
export function BranchIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <circle cx="4" cy="3.5" r="1.8" />
      <circle cx="4" cy="12.5" r="1.8" />
      <circle cx="12" cy="6" r="1.8" />
      <path d="M4 5.3v5.4M12 7.8c0 2-1.6 2.9-4 2.9" />
    </svg>
  );
}

export function FilesIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <path d="M9 1.5H4a1 1 0 0 0-1 1v11a1 1 0 0 0 1 1h8a1 1 0 0 0 1-1V5.5z" />
      <path d="M9 1.5v4h4" />
    </svg>
  );
}

export function AccountsIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <circle cx="8" cy="5.5" r="2.5" />
      <path d="M3 13.5c0-2.2 2.2-3.5 5-3.5s5 1.3 5 3.5" />
    </svg>
  );
}

/** A gauge: what's left, which is the question this answers. */
export function UsageIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <path d="M2.5 11.5a6 6 0 1 1 11 0" />
      <path d="M8 11.5 11 6.8" />
    </svg>
  );
}

/** A gear — which is what this always meant to be.
 *
 * What was here was a hub with eight detached radial strokes, i.e. a sun. The
 * difference isn't decorative: rays that don't touch anything read as light,
 * and teeth that interrupt a closed contour read as a mechanism. So the outline
 * is one continuous path that steps out and back six times, and the hub sits
 * inside it rather than floating free of it.
 *
 * Six teeth, not eight: at 16px with a 1.5 stroke the gaps between eight fill
 * in and the whole thing turns back into a blob. */
export function SettingsIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <path d="M6.2 4.15 L6.75 2.59 L9.25 2.59 L9.8 4.15 L10.44 4.52 L12.06 4.21 L13.31 6.38 L12.23 7.63 L12.23 8.37 L13.31 9.62 L12.06 11.79 L10.44 11.48 L9.8 11.85 L9.25 13.41 L6.75 13.41 L6.2 11.85 L5.56 11.48 L3.94 11.79 L2.69 9.62 L3.77 8.37 L3.77 7.63 L2.69 6.38 L3.94 4.21 L5.56 4.52 Z" />
      <circle cx="8" cy="8" r="2.1" />
    </svg>
  );
}

/** The overflow menu trigger — three bars, nothing fancier. Only shows up
 * once the nav row no longer fits (`AppTopBar`'s narrow breakpoint). */
export function MenuIcon() {
  return (
    <svg {...base} aria-hidden="true">
      <path d="M2.5 4.5h11M2.5 8h11M2.5 11.5h11" />
    </svg>
  );
}
