import { Component, useEffect, useState, type ReactNode } from 'react';
import { Alert, AlertDescription } from './ui/alert';
import { Button } from './ui/button';

/**
 * Makes frontend failures visible without devtools access — a silent JS
 * exception (render crash, unhandled promise rejection, global error) would
 * otherwise look identical to "the button did nothing," which is exactly
 * the class of bug this app has been debugging blind. Catches what
 * `window.onerror`/`unhandledrejection` can (runtime errors) plus a class
 * error boundary for render-time errors (React doesn't route those through
 * `window.onerror`).
 */

type ToastKind = 'error' | 'success';

interface ToastEntry {
  id: number;
  message: string;
  kind: ToastKind;
}

let nextId = 1;
type Listener = (entry: ToastEntry) => void;
/** Any screen can push into the same floating toast stack that catches
 * unhandled JS errors — a plain module-level pub/sub rather than a Context,
 * since there's exactly one consumer (`ToastStack`, always mounted once at
 * the app root) and no need to thread a provider through every screen. */
const listeners = new Set<Listener>();

/** Call from any screen's `catch` block instead of a local error-banner
 * state — every failure then looks and behaves the same way (dismissible,
 * stacked bottom-left), instead of each screen reinventing its own inline
 * banner. */
export function toastError(message: string) {
  const entry: ToastEntry = { id: nextId++, message, kind: 'error' };
  listeners.forEach((l) => l(entry));
}

/** Same stack, styled for a quick confirmation (e.g. "Fetched") — auto-
 * dismisses after a few seconds, unlike an error toast which stays until
 * the user closes it. */
export function toastSuccess(message: string) {
  const entry: ToastEntry = { id: nextId++, message, kind: 'success' };
  listeners.forEach((l) => l(entry));
}

function useToastStack() {
  const [entries, setEntries] = useState<ToastEntry[]>([]);

  useEffect(() => {
    const push = (entry: ToastEntry) => {
      setEntries((prev) => [...prev.slice(-4), entry]);
      if (entry.kind === 'success') {
        setTimeout(() => setEntries((prev) => prev.filter((e) => e.id !== entry.id)), 3000);
      }
    };
    listeners.add(push);

    const onError = (event: ErrorEvent) => {
      push({ id: nextId++, kind: 'error', message: `${event.message}${event.filename ? ` (${event.filename}:${event.lineno})` : ''}` });
    };
    const onRejection = (event: PromiseRejectionEvent) => {
      const reason = event.reason instanceof Error ? event.reason.message : String(event.reason);
      push({ id: nextId++, kind: 'error', message: `Unhandled promise rejection: ${reason}` });
    };
    window.addEventListener('error', onError);
    window.addEventListener('unhandledrejection', onRejection);
    return () => {
      listeners.delete(push);
      window.removeEventListener('error', onError);
      window.removeEventListener('unhandledrejection', onRejection);
    };
  }, []);

  const dismiss = (id: number) => setEntries((prev) => prev.filter((e) => e.id !== id));
  return { entries, dismiss };
}

function ToastStack() {
  const { entries, dismiss } = useToastStack();
  if (entries.length === 0) return null;

  return (
    <div
      style={{
        position: 'fixed',
        bottom: 12,
        left: 12,
        right: 12,
        zIndex: 9999,
        display: 'flex',
        flexDirection: 'column',
        gap: 6,
        pointerEvents: 'none',
      }}
    >
      {entries.map((e) => (
        <Alert
          key={e.id}
          variant={e.kind === 'error' ? 'destructive' : 'default'}
          className="flex items-start gap-2.5 shadow-lg"
          style={{
            pointerEvents: 'auto',
            font: "500 11px 'JetBrains Mono', monospace",
            ...(e.kind === 'success' ? { borderColor: 'var(--grn)', color: 'var(--grn)' } : {}),
          }}
        >
          <AlertDescription className="flex-1 text-inherit" style={{ wordBreak: 'break-word' }}>
            {e.kind === 'error' ? '⚠' : '✓'} {e.message}
          </AlertDescription>
          <Button variant="ghost" size="icon-sm" onClick={() => dismiss(e.id)}>
            ✕
          </Button>
        </Alert>
      ))}
    </div>
  );
}

interface BoundaryState {
  error: Error | null;
}

class RenderErrorBoundary extends Component<{ children: ReactNode }, BoundaryState> {
  state: BoundaryState = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: { componentStack?: string }) {
    // eslint-disable-next-line no-console
    console.error('Render error caught by RenderErrorBoundary:', error, info);
  }

  render() {
    if (this.state.error) {
      return (
        <div
          style={{
            width: '100%',
            height: '100vh',
            background: '#0e0f14',
            color: '#e8e8f2',
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            gap: 12,
            fontFamily: "'IBM Plex Sans', sans-serif",
            padding: 24,
            textAlign: 'center',
          }}
        >
          <span style={{ font: "600 16px 'IBM Plex Sans', sans-serif" }}>Something crashed while rendering</span>
          <pre
            style={{
              font: "400 11px 'JetBrains Mono', monospace",
              color: '#e56a7b',
              maxWidth: 640,
              whiteSpace: 'pre-wrap',
              background: '#171822',
              padding: 12,
              borderRadius: 8,
              border: '1px solid #262838',
            }}
          >
            {this.state.error.message}
          </pre>
          <Button
            onClick={() => this.setState({ error: null })}
            style={{
              font: "600 12px 'IBM Plex Sans', sans-serif",
              padding: '8px 18px',
              borderRadius: 7,
              background: '#b48cff',
              color: '#15151c',
            }}
          >
            Try to recover
          </Button>
        </div>
      );
    }
    return this.props.children;
  }
}

export function AppErrorOverlay({ children }: { children: ReactNode }) {
  return (
    <RenderErrorBoundary>
      {children}
      <ToastStack />
    </RenderErrorBoundary>
  );
}
