/** Small colored letter-badge per file extension for the file tree — same
 * "letter + color badge" pattern already used for provider/status badges,
 * without pulling in an icon font/library. */
interface FileIconMeta {
  letter: string;
  color: string;
}

const EXTENSION_ICONS: Record<string, FileIconMeta> = {
  ts: { letter: 'TS', color: 'var(--acc)' },
  tsx: { letter: 'TS', color: 'var(--acc)' },
  js: { letter: 'JS', color: 'var(--amber)' },
  jsx: { letter: 'JS', color: 'var(--amber)' },
  mjs: { letter: 'JS', color: 'var(--amber)' },
  rs: { letter: 'RS', color: 'var(--gemini)' },
  css: { letter: 'CS', color: 'var(--teal)' },
  scss: { letter: 'CS', color: 'var(--teal)' },
  json: { letter: '{}', color: 'var(--grn)' },
  md: { letter: 'MD', color: 'var(--mut)' },
  py: { letter: 'PY', color: 'var(--grn)' },
  html: { letter: 'HT', color: 'var(--red)' },
  yaml: { letter: 'YM', color: 'var(--dim)' },
  yml: { letter: 'YM', color: 'var(--dim)' },
  toml: { letter: 'TM', color: 'var(--dim)' },
  sql: { letter: 'SQ', color: 'var(--amber)' },
  sh: { letter: 'SH', color: 'var(--mut)' },
};

const DEFAULT_ICON: FileIconMeta = { letter: '·', color: 'var(--dim)' };

export function fileIcon(name: string): FileIconMeta {
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return DEFAULT_ICON;
  const ext = name.slice(dot + 1).toLowerCase();
  return EXTENSION_ICONS[ext] ?? DEFAULT_ICON;
}
