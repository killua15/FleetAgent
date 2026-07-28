import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { applyWindowGlass, readUnfocusedTint, writeUnfocusedTint } from './glass';

export type Theme = 'dark' | 'light' | 'glass' | 'system';
export type ResolvedTheme = 'dark' | 'light' | 'glass';

const STORAGE_KEY = 'hangar.theme';
/// Key used before the Fleet → Hangar rename. Read once as a fallback so the
/// rename doesn't silently reset a theme the user picked.
const LEGACY_STORAGE_KEY = 'fleet.theme';

interface ThemeContextValue {
  /** The user's stored preference — may be `'system'`. */
  theme: Theme;
  /** What's actually applied to the DOM (`'system'` resolved to dark/light). */
  resolvedTheme: ResolvedTheme;
  /** Cycles dark↔light only (Cmd/Ctrl+Shift+L) — leaves `'system'` alone if
   * that's not the current preference, matching the shortcut's pre-existing
   * two-state behavior; use `setTheme('system')` to opt back in. */
  toggleTheme: () => void;
  setTheme: (theme: Theme) => void;
  /** How much tint the window keeps when it isn't focused, 0-1. Only has any
   * effect in the glass theme. */
  unfocusedTint: number;
  setUnfocusedTint: (value: number) => void;
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function readStoredTheme(): Theme {
  const stored = window.localStorage.getItem(STORAGE_KEY) ?? window.localStorage.getItem(LEGACY_STORAGE_KEY);
  return stored === 'light' || stored === 'dark' || stored === 'glass' || stored === 'system' ? stored : 'dark';
}

function systemPrefersDark(): boolean {
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(readStoredTheme);
  const [systemDark, setSystemDark] = useState(systemPrefersDark);
  const [unfocusedTint, setUnfocusedTintState] = useState(readUnfocusedTint);

  useEffect(() => {
    const mq = window.matchMedia('(prefers-color-scheme: dark)');
    const handler = (e: MediaQueryListEvent) => setSystemDark(e.matches);
    mq.addEventListener('change', handler);
    return () => mq.removeEventListener('change', handler);
  }, []);

  const resolvedTheme: ResolvedTheme = theme === 'system' ? (systemDark ? 'dark' : 'light') : theme;

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', resolvedTheme);
    window.localStorage.setItem(STORAGE_KEY, theme);
    // The native side of glass: without the vibrancy material the CSS tints
    // have nothing to sit on top of.
    void applyWindowGlass(resolvedTheme === 'glass');
  }, [theme, resolvedTheme]);

  // The window fades when you look away and comes back when you return. Driven
  // by a data attribute rather than by writing styles here, so the whole
  // behaviour stays described in tokens.css where the rest of the theme lives.
  useEffect(() => {
    const root = document.documentElement;
    const setFocused = (focused: boolean) => root.setAttribute('data-window-focused', String(focused));
    setFocused(true);

    const win = getCurrentWindow();
    const unlisten = win.onFocusChanged(({ payload }) => setFocused(payload));
    // A plain browser (`npm run dev` outside Tauri) never fires that event, so
    // the DOM events keep the behaviour visible while developing.
    const onFocus = () => setFocused(true);
    const onBlur = () => setFocused(false);
    window.addEventListener('focus', onFocus);
    window.addEventListener('blur', onBlur);
    return () => {
      unlisten.then((f) => f()).catch(() => {});
      window.removeEventListener('focus', onFocus);
      window.removeEventListener('blur', onBlur);
    };
  }, []);

  useEffect(() => {
    document.documentElement.style.setProperty('--glass-tint-unfocused', String(unfocusedTint));
  }, [unfocusedTint]);

  const setTheme = (next: Theme) => setThemeState(next);
  const setUnfocusedTint = (value: number) => {
    writeUnfocusedTint(value);
    setUnfocusedTintState(value);
  };
  const toggleTheme = () => {
    const current = theme === 'system' ? resolvedTheme : theme;
    setThemeState(current === 'dark' ? 'light' : 'dark');
  };

  return (
    <ThemeContext.Provider value={{ theme, resolvedTheme, toggleTheme, setTheme, unfocusedTint, setUnfocusedTint }}>{children}</ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error('useTheme must be used within a ThemeProvider');
  return ctx;
}
