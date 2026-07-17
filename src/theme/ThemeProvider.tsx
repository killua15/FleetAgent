import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';

export type Theme = 'dark' | 'light' | 'glass' | 'system';
export type ResolvedTheme = 'dark' | 'light' | 'glass';

const STORAGE_KEY = 'fleet.theme';

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
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

function readStoredTheme(): Theme {
  const stored = window.localStorage.getItem(STORAGE_KEY);
  return stored === 'light' || stored === 'dark' || stored === 'glass' || stored === 'system' ? stored : 'dark';
}

function systemPrefersDark(): boolean {
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [theme, setThemeState] = useState<Theme>(readStoredTheme);
  const [systemDark, setSystemDark] = useState(systemPrefersDark);

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
  }, [theme, resolvedTheme]);

  const setTheme = (next: Theme) => setThemeState(next);
  const toggleTheme = () => {
    const current = theme === 'system' ? resolvedTheme : theme;
    setThemeState(current === 'dark' ? 'light' : 'dark');
  };

  return (
    <ThemeContext.Provider value={{ theme, resolvedTheme, toggleTheme, setTheme }}>{children}</ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error('useTheme must be used within a ThemeProvider');
  return ctx;
}
