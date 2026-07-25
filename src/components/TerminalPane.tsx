import { useEffect, useRef } from 'react';
import { Channel } from '@tauri-apps/api/core';
import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { invoke } from '../lib/invoke';
import { useTheme } from '../theme/ThemeProvider';
import type { TerminalChunk } from '../types/terminal';
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

    const dataSub = term.onData((data) => {
      invoke('write_terminal', { id: terminalId, data }).catch(() => {});
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

  return <div ref={hostRef} className={className} />;
}
