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

interface CaughtError {
  id: number;
  message: string;
}

let nextId = 1;

function useGlobalErrorCapture() {
  const [errors, setErrors] = useState<CaughtError[]>([]);

  useEffect(() => {
    const push = (message: string) => {
      setErrors((prev) => [...prev.slice(-4), { id: nextId++, message }]);
    };
    const onError = (event: ErrorEvent) => {
      push(`${event.message}${event.filename ? ` (${event.filename}:${event.lineno})` : ''}`);
    };
    const onRejection = (event: PromiseRejectionEvent) => {
      const reason = event.reason instanceof Error ? event.reason.message : String(event.reason);
      push(`Unhandled promise rejection: ${reason}`);
    };
    window.addEventListener('error', onError);
    window.addEventListener('unhandledrejection', onRejection);
    return () => {
      window.removeEventListener('error', onError);
      window.removeEventListener('unhandledrejection', onRejection);
    };
  }, []);

  const dismiss = (id: number) => setErrors((prev) => prev.filter((e) => e.id !== id));
  return { errors, dismiss };
}

function ErrorBanner() {
  const { errors, dismiss } = useGlobalErrorCapture();
  if (errors.length === 0) return null;

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
      {errors.map((e) => (
        <Alert
          key={e.id}
          variant="destructive"
          className="flex items-start gap-2.5 shadow-lg"
          style={{ pointerEvents: 'auto', font: "500 11px 'JetBrains Mono', monospace" }}
        >
          <AlertDescription className="flex-1 text-inherit" style={{ wordBreak: 'break-word' }}>
            ⚠ {e.message}
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
      <ErrorBanner />
    </RenderErrorBoundary>
  );
}
