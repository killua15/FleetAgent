import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from 'react';

/**
 * Screen names mirror the Hangar mockup files (`<Name>.dc.html`). Not all are
 * implemented yet — screens land phase by phase; this union is declared up
 * front so navigation calls compile against the full map as it fills in.
 */
export type ScreenName =
  | 'landing'
  | 'add-account'
  | 'account-list'
  | 'main-view'
  | 'agent-panel'
  | 'split-view'
  | 'exec-run'
  | 'rotation-settings'
  | 'project-settings'
  | 'skill-settings'
  | 'task-settings'
  | 'task-board'
  | 'shared-memory'
  | 'graph-groups'
  | 'git-panel'
  | 'files'
  | 'queue'
  | 'git-history'
  | 'changes'
  | 'pr-modal'
  | 'pr-reviews'
  | 'general-settings'
  | 'forge-settings'
  | 'tree-canvas'
  | 'tree-columns'
  | 'tree-outline'
  | 'terminals'
  | 'command-palette';

export interface NavEntry {
  screen: ScreenName;
  params?: Record<string, unknown>;
}

interface NavigationContextValue {
  current: NavEntry;
  history: NavEntry[];
  navigate: (screen: ScreenName, params?: Record<string, unknown>) => void;
  back: () => void;
  canGoBack: boolean;
}

const NavigationContext = createContext<NavigationContextValue | null>(null);

export function NavigationProvider({ children }: { children: ReactNode }) {
  const [stack, setStack] = useState<NavEntry[]>([{ screen: 'landing' }]);

  const navigate = useCallback((screen: ScreenName, params?: Record<string, unknown>) => {
    setStack((prev) => [...prev, { screen, params }]);
  }, []);

  const back = useCallback(() => {
    setStack((prev) => (prev.length > 1 ? prev.slice(0, -1) : prev));
  }, []);

  const value = useMemo<NavigationContextValue>(
    () => ({
      current: stack[stack.length - 1],
      history: stack,
      navigate,
      back,
      canGoBack: stack.length > 1,
    }),
    [stack, navigate, back],
  );

  return <NavigationContext.Provider value={value}>{children}</NavigationContext.Provider>;
}

export function useNavigation(): NavigationContextValue {
  const ctx = useContext(NavigationContext);
  if (!ctx) throw new Error('useNavigation must be used within a NavigationProvider');
  return ctx;
}
