/** Line-based diff (LCS/dynamic-programming, same idea as `git diff`) used by
 * both the sidecar's Diff tab and the inline timeline preview — replaces the
 * earlier "dump every old line, then every new line" rendering with real
 * aligned context, additions and removals. */
export type DiffLine =
  | { type: 'context'; text: string; oldLine: number; newLine: number }
  | { type: 'add'; text: string; newLine: number }
  | { type: 'remove'; text: string; oldLine: number }
  | { type: 'collapsed'; count: number };

type RawDiffEntry = { type: 'context' | 'add' | 'remove'; text: string };

/** Above this, an O(n*m) LCS table gets too large to build cheaply — falls
 * back to "all removed, then all added" rather than freezing the UI on a
 * pathologically large Write. */
const MAX_LCS_CELLS = 500_000;

function splitLines(text: string): string[] {
  if (text === '') return [];
  const trimmed = text.endsWith('\n') ? text.slice(0, -1) : text;
  return trimmed.split('\n');
}

function computeLcsDiff(oldLines: string[], newLines: string[]): RawDiffEntry[] {
  const n = oldLines.length;
  const m = newLines.length;

  if (n * m > MAX_LCS_CELLS) {
    return [
      ...oldLines.map((text): RawDiffEntry => ({ type: 'remove', text })),
      ...newLines.map((text): RawDiffEntry => ({ type: 'add', text })),
    ];
  }

  const dp: number[][] = Array.from({ length: n + 1 }, () => new Array(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = oldLines[i] === newLines[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }

  const result: RawDiffEntry[] = [];
  let i = 0;
  let j = 0;
  while (i < n && j < m) {
    if (oldLines[i] === newLines[j]) {
      result.push({ type: 'context', text: oldLines[i] });
      i++;
      j++;
    } else if (dp[i + 1][j] >= dp[i][j + 1]) {
      result.push({ type: 'remove', text: oldLines[i] });
      i++;
    } else {
      result.push({ type: 'add', text: newLines[j] });
      j++;
    }
  }
  while (i < n) {
    result.push({ type: 'remove', text: oldLines[i] });
    i++;
  }
  while (j < m) {
    result.push({ type: 'add', text: newLines[j] });
    j++;
  }
  return result;
}

/** `contextLines` bounds how many unchanged lines surround each change
 * before the rest of a long unchanged run collapses into a single marker —
 * pass `Infinity` to keep every line (used by `diffStats`, where collapsing
 * would just be wasted work). */
export function diffLines(
  oldTextRaw: string | null | undefined,
  newTextRaw: string | null | undefined,
  contextLines = 2,
): DiffLine[] {
  const oldLinesArr = splitLines(oldTextRaw ?? '');
  const newLinesArr = splitLines(newTextRaw ?? '');
  const raw = computeLcsDiff(oldLinesArr, newLinesArr);

  let oldLine = 1;
  let newLine = 1;
  const numbered: DiffLine[] = raw.map((entry) => {
    if (entry.type === 'context') {
      const line: DiffLine = { type: 'context', text: entry.text, oldLine, newLine };
      oldLine++;
      newLine++;
      return line;
    }
    if (entry.type === 'remove') {
      const line: DiffLine = { type: 'remove', text: entry.text, oldLine };
      oldLine++;
      return line;
    }
    const line: DiffLine = { type: 'add', text: entry.text, newLine };
    newLine++;
    return line;
  });

  if (!Number.isFinite(contextLines)) return numbered;

  const collapsed: DiffLine[] = [];
  let i = 0;
  while (i < numbered.length) {
    if (numbered[i].type !== 'context') {
      collapsed.push(numbered[i]);
      i++;
      continue;
    }
    let j = i;
    while (j < numbered.length && numbered[j].type === 'context') j++;
    const runLength = j - i;
    const keepBefore = i === 0 ? 0 : contextLines;
    const keepAfter = j === numbered.length ? 0 : contextLines;
    if (runLength <= keepBefore + keepAfter) {
      for (let k = i; k < j; k++) collapsed.push(numbered[k]);
    } else {
      for (let k = i; k < i + keepBefore; k++) collapsed.push(numbered[k]);
      collapsed.push({ type: 'collapsed', count: runLength - keepBefore - keepAfter });
      for (let k = j - keepAfter; k < j; k++) collapsed.push(numbered[k]);
    }
    i = j;
  }
  return collapsed;
}

export function diffStats(
  oldText: string | null | undefined,
  newText: string | null | undefined,
): { additions: number; removals: number } {
  const lines = diffLines(oldText, newText, Infinity);
  let additions = 0;
  let removals = 0;
  for (const line of lines) {
    if (line.type === 'add') additions++;
    else if (line.type === 'remove') removals++;
  }
  return { additions, removals };
}
