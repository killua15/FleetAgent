/** Extension → display language name + Prism grammar id, for the FILE VIEW's
 * language tag and syntax highlighting — same extension set as `fileIcon.ts`,
 * just extended with the full name and the grammar `prism-react-renderer`
 * needs (its bundled build only ships a subset of Prism's languages; `sh`/
 * `toml` fall back to `'text'` — unhighlighted, but still line-numbered and
 * monospaced like everything else, never a broken render). */
interface LanguageMeta {
  name: string;
  prism: string;
}

const LANGUAGE_BY_EXTENSION: Record<string, LanguageMeta> = {
  ts: { name: 'TypeScript', prism: 'typescript' },
  tsx: { name: 'TypeScript (JSX)', prism: 'tsx' },
  js: { name: 'JavaScript', prism: 'javascript' },
  jsx: { name: 'JavaScript (JSX)', prism: 'jsx' },
  mjs: { name: 'JavaScript', prism: 'javascript' },
  rs: { name: 'Rust', prism: 'rust' },
  css: { name: 'CSS', prism: 'css' },
  scss: { name: 'SCSS', prism: 'css' },
  json: { name: 'JSON', prism: 'json' },
  md: { name: 'Markdown', prism: 'markdown' },
  py: { name: 'Python', prism: 'python' },
  html: { name: 'HTML', prism: 'markup' },
  yaml: { name: 'YAML', prism: 'yaml' },
  yml: { name: 'YAML', prism: 'yaml' },
  toml: { name: 'TOML', prism: 'text' },
  sql: { name: 'SQL', prism: 'sql' },
  sh: { name: 'Shell', prism: 'text' },
};

const DEFAULT_LANGUAGE: LanguageMeta = { name: 'Plain text', prism: 'text' };

export function languageFor(name: string): LanguageMeta {
  const dot = name.lastIndexOf('.');
  if (dot <= 0) return DEFAULT_LANGUAGE;
  const ext = name.slice(dot + 1).toLowerCase();
  return LANGUAGE_BY_EXTENSION[ext] ?? DEFAULT_LANGUAGE;
}
