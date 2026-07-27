import { useEffect, useRef, useState } from 'react';
import { Channel } from '@tauri-apps/api/core';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { invoke } from '../lib/invoke';
import { toastError } from './ErrorOverlay';
import { useTheme } from '../theme/ThemeProvider';
import type { TerminalChunk } from '../types/terminal';
import styles from './TerminalPane.module.css';
import '@xterm/xterm/css/xterm.css';

/** Raw PTY bytes arrive base64-encoded (see `types/terminal.ts`); xterm.js
 * takes a `Uint8Array` directly, so nothing ever has to guess at an encoding. */
function decodeChunk(base64: string): Uint8Array {
  const binary = window.atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/** Reads the app's own design tokens off the document so the terminal follows
 * whichever theme is active — including `glass`, which a hardcoded
 * dark/light pair would have missed entirely. */
function themeFromTokens(): Record<string, string> {
  const css = getComputedStyle(document.documentElement);
  const token = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  // In glass, `--panel` is already an rgba tint, so xterm composites over the
  // window's own translucency and the desktop shows through the terminal too.
  return {
    background: token('--panel', '#171822'),
    foreground: token('--txt', '#e8e8f2'),
    cursor: token('--acc', '#b48cff'),
    cursorAccent: token('--panel', '#171822'),
    selectionBackground: token('--accD', 'rgba(180, 140, 255, 0.25)'),
    black: token('--bg', '#0e0f14'),
    red: token('--red', '#e56a7b'),
    green: token('--grn', '#5fd39a'),
    yellow: token('--amber', '#e0a458'),
    blue: token('--gemini', '#7aa2ff'),
    magenta: token('--acc', '#b48cff'),
    cyan: token('--teal', '#3ecfc0'),
    white: token('--txt', '#e8e8f2'),
    brightBlack: token('--dim', '#61627a'),
  };
}

/** A single live shell. The PTY itself lives in Rust and outlives this
 * component — unmounting only detaches the output stream (`detach_terminal`),
 * so navigating away and back replays the scrollback and reconnects to the
 * same still-running process rather than starting a new one. */
export function TerminalPane({ terminalId, className }: { terminalId: string; className?: string }) {
  const [dropping, setDropping] = useState(false);
  const [saving, setSaving] = useState(false);
  const hostRef = useRef<HTMLDivElement>(null);
  const { resolvedTheme } = useTheme();
  const termRef = useRef<Terminal | null>(null);

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    const term = new Terminal({
      fontFamily: "'JetBrains Mono', ui-monospace, monospace",
      fontSize: 12,
      theme: themeFromTokens(),
      cursorBlink: true,
      // Without this xterm paints an opaque background regardless of the theme
      // colour, and the terminals — the largest surface in the app — would be
      // the one place the glass stops.
      allowTransparency: true,
      // Matches the backend's own scrollback cap — keeping far more here than
      // Rust can replay would only be visible until the first remount.
      scrollback: 5000,
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    termRef.current = term;

    let disposed = false;
    // Guards against a fit() that runs after unmount (the ResizeObserver can
    // fire during teardown) invoking a command for a detached terminal.
    const syncSize = () => {
      if (disposed) return;
      try {
        fit.fit();
      } catch {
        // fit() throws if the host has no layout yet (display:none, zero
        // height mid-transition) — the next observer tick will retry.
        return;
      }
      invoke('resize_terminal', { id: terminalId, cols: term.cols, rows: term.rows }).catch(() => {});
    };

    const channel = new Channel<TerminalChunk>();
    channel.onmessage = (chunk) => {
      if (!disposed) term.write(decodeChunk(chunk.base64));
    };

    // `attach_terminal` installs the channel and returns the scrollback in one
    // atomic step, so nothing printed in between is lost.
    invoke<string>('attach_terminal', { id: terminalId, channel })
      .then((scrollback) => {
        if (disposed) return;
        if (scrollback) term.write(decodeChunk(scrollback));
        syncSize();
        term.focus();
      })
      .catch(() => {
        if (!disposed) term.write('\r\n\x1b[31mThis terminal is no longer available.\x1b[0m\r\n');
      });

    const send = (data: string) => {
      invoke('write_terminal', { id: terminalId, data }).catch(() => {});
    };

    const dataSub = term.onData(send);

    // Shift+Enter (and Option+Enter) as a newline instead of a send.
    //
    // In a real terminal both send exactly what Enter sends, which is why
    // Claude Code ships `/terminal-setup` to reconfigure iTerm2 and VS Code to
    // emit `ESC CR` instead. Here the terminal is ours, so there's nothing to
    // ask the user to configure — we emit it directly.
    term.attachCustomKeyEventHandler((event) => {
      if (event.type !== 'keydown' || event.key !== 'Enter') return true;
      if (!event.shiftKey && !event.altKey) return true;
      // Ctrl+Enter and Cmd+Enter mean other things in some CLIs; only claim
      // the two combinations that conventionally mean "newline".
      if (event.ctrlKey || event.metaKey) return true;
      event.preventDefault();
      send('\x1b\r');
      return false;
    });

    // The PTY has to be told the real size or anything full-screen (`vim`,
    // `htop`, a progress bar) draws against the wrong width.
    const observer = new ResizeObserver(() => syncSize());
    observer.observe(host);

    return () => {
      disposed = true;
      observer.disconnect();
      dataSub.dispose();
      // Stop the stream but leave the shell running — this is an unmount, not
      // a close. `close_terminal` is what actually kills it.
      invoke('detach_terminal', { id: terminalId }).catch(() => {});
      term.dispose();
      termRef.current = null;
    };
  }, [terminalId]);

  // Re-theming an existing terminal in place, rather than keying the effect
  // above on the theme, keeps the scrollback and the running process intact
  // across a light/dark toggle.
  useEffect(() => {
    if (termRef.current) termRef.current.options.theme = themeFromTokens();
  }, [resolvedTheme]);

  /** Dropping a file writes its *path* into the terminal, which is what a CLI
   * expects — the same thing iTerm does, and the reason dragging a screenshot
   * into an agent works there and did nothing here.
   *
   * `preventDefault` on dragover is not optional: without it the webview
   * navigates to the dropped image and the user loses the whole app. */
  const onDrop = async (event: React.DragEvent<HTMLDivElement>) => {
    event.preventDefault();
    setDropping(false);

    const files = Array.from(event.dataTransfer.files);
    if (files.length === 0) {
      // Dragged text (a path from another app, a URL) — insert it as typed.
      const text = event.dataTransfer.getData('text/plain');
      if (text) invoke('write_terminal', { id: terminalId, data: text }).catch(() => {});
      return;
    }

    setSaving(true);
    try {
      const paths: string[] = [];
      for (const file of files) {
        // A directory arrives as a File with no type and no readable body;
        // saying so beats writing a path the agent can't read.
        if (file.size === 0 && file.type === '') {
          toastError(`${file.name} looks like a folder — drop a file instead`);
          continue;
        }
        const data = Array.from(new Uint8Array(await file.arrayBuffer()));
        // The attachments dir is namespaced by id; a terminal id works as well
        // as a session id and keeps one terminal's drops together.
        const path = await invoke<string>('save_attachment', { sessionId: terminalId, fileName: file.name, data });
        paths.push(path);
      }
      if (paths.length > 0) {
        // Quoted: a screenshot filename on macOS has spaces in it.
        invoke('write_terminal', { id: terminalId, data: paths.map((p) => `'${p.replace(/'/g, "'\\''")}' `).join('') }).catch(() => {});
      }
    } catch (e) {
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // Wrapper rather than putting these on the host itself: xterm.js owns the
  // host's children, and hanging a badge inside it means competing with a
  // library for the same DOM.
  return (
    <div
      className={`${styles.wrap} ${className ?? ''}`}
      onDragOver={(e) => {
        e.preventDefault();
        setDropping(true);
      }}
      onDragLeave={() => setDropping(false)}
      onDrop={onDrop}
      data-dropping={dropping || undefined}
    >
      <div ref={hostRef} className={styles.host} />
      {/* Copying a 4 MB screenshot isn't instant, and a pane that doesn't
          react to a drop looks broken. */}
      {saving && <span className={styles.dropBadge}>saving…</span>}
    </div>
  );
}
