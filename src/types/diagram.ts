/** One diagram an agent delivered for a project, from `list_project_diagrams`.
 *
 * Read out of the page's own head — archify writes a `<title>` and a
 * `<meta name="generator">` in the first bytes — so nothing had to be recorded
 * at delivery time. It could not have been: archify writes through `Bash`, and
 * the `file_edits` Hangar keeps only sees `Edit`, `Write` and `MultiEdit`. */
export interface Diagram {
  /** The file name, and what `open_diagram` takes back. Never a path. */
  fileName: string;
  /** The page's `<title>`, falling back to the file name when it has none. */
  title: string;
  /** From `<meta name="generator">`, e.g. `archify 2.16.0`. `null` for an HTML
   * file that does not say — listed anyway rather than hidden. */
  generator: string | null;
  bytes: number;
  /** RFC 3339, or `null` when the filesystem would not say. */
  modified: string | null;
}
