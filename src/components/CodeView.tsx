import { useEffect, useRef } from 'react';
import { EditorState, type Extension } from '@codemirror/state';
import { EditorView, lineNumbers, highlightActiveLine } from '@codemirror/view';
import { MergeView, unifiedMergeView } from '@codemirror/merge';
import { oneDark } from '@codemirror/theme-one-dark';
import { javascript } from '@codemirror/lang-javascript';
import { rust } from '@codemirror/lang-rust';
import { json } from '@codemirror/lang-json';
import { css } from '@codemirror/lang-css';
import { html } from '@codemirror/lang-html';
import { markdown } from '@codemirror/lang-markdown';
import { useTheme } from '../theme/ThemeProvider';
import styles from './CodeView.module.css';

/** Language support by extension. Deliberately short: these cover what this
 * codebase and the ones it works on are written in, and an unknown extension
 * degrades to plain text with line numbers rather than to an error. */
function languageFor(path: string): Extension[] {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  switch (ext) {
    case 'ts':
    case 'tsx':
    case 'js':
    case 'jsx':
    case 'mjs':
    case 'cjs':
      return [javascript({ typescript: ext.startsWith('ts'), jsx: ext.endsWith('x') })];
    case 'rs':
      return [rust()];
    case 'json':
      return [json()];
    case 'css':
      return [css()];
    case 'html':
      return [html()];
    case 'md':
      return [markdown()];
    default:
      return [];
  }
}

/** Read-only code viewer, with an optional diff against `original` — inline
 * (`unified`, GitHub Desktop's way) or before | after columns (`split`, the
 * IDE's way).
 *
 * CodeMirror rather than Monaco on purpose: this screen exists to *read* what an
 * agent changed, and Monaco's extra weight (~90 MB unpacked against ~1 MB here,
 * plus web workers) buys editing and IntelliSense — the exact surface Hangar
 * decided not to compete on. CodeMirror is a real editor too, so the ceiling
 * stays where it was; only the payload shrank.
 */
export function CodeView({
  path,
  text,
  original,
  split = false,
}: {
  path: string;
  text: string;
  original?: string | null;
  split?: boolean;
}) {
  const host = useRef<HTMLDivElement>(null);
  // `resolvedTheme` and not `theme`: the latter can be 'system', which is a
  // preference, not a colour scheme.
  const { resolvedTheme } = useTheme();

  useEffect(() => {
    if (!host.current) return;
    const extensions: Extension[] = [
      lineNumbers(),
      highlightActiveLine(),
      EditorView.editable.of(false),
      EditorState.readOnly.of(true),
      EditorView.lineWrapping,
      ...languageFor(path),
    ];
    if (resolvedTheme === 'dark') extensions.push(oneDark);
    // Only when there's something to compare against: an untracked file has no
    // committed version, and an empty diff view would imply it's unchanged.
    const hasDiff = original != null && original !== text;

    // Both columns share the same read-only setup, so the two sides of a split
    // look like one file at two moments rather than two different viewers.
    if (hasDiff && split) {
      const view = new MergeView({
        a: { doc: original, extensions },
        b: { doc: text, extensions },
        parent: host.current,
        collapseUnchanged: { margin: 3, minSize: 6 },
      });
      return () => view.destroy();
    }
    if (hasDiff) {
      extensions.push(unifiedMergeView({ original, mergeControls: false }));
    }

    const view = new EditorView({ state: EditorState.create({ doc: text, extensions }), parent: host.current });
    return () => view.destroy();
  }, [path, text, original, split, resolvedTheme]);

  return <div ref={host} className={styles.host} />;
}
