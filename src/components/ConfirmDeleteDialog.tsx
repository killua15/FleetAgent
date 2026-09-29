import { useState, type MouseEvent, type ReactNode } from 'react';
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from './ui/alert-dialog';
import { Spinner } from './Spinner';

interface ConfirmDeleteDialogProps {
  trigger: ReactNode;
  title: string;
  description: string;
  confirmLabel?: string;
  onConfirm: () => void | Promise<void>;
  /** Extra content under the description — an option that changes what the
   * delete takes with it. */
  children?: ReactNode;
}

/**
 * Shared confirm-delete affordance — replaces the click-to-reveal-✓/✕ pattern
 * that was reimplemented with duplicated inline styles in AccountList,
 * AgentPanel, and MainView (twice).
 *
 * Deletes routinely touch disk (worktrees, credentials) and can take a
 * beat — this owns its own pending state so every caller gets a spinner and
 * a disabled Cancel "for free" instead of each screen re-inventing one (or,
 * as before, not having one at all: the dialog would otherwise close the
 * instant Delete is clicked — Radix's default — leaving the click with no
 * visible effect until the op finished or silently failed).
 */
export function ConfirmDeleteDialog({ trigger, title, description, confirmLabel = 'Delete', onConfirm, children }: ConfirmDeleteDialogProps) {
  const [open, setOpen] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleConfirm = async (e: MouseEvent<HTMLButtonElement>) => {
    e.preventDefault(); // keep the dialog open — we close it ourselves once the delete actually finishes
    setPending(true);
    setError(null);
    try {
      await onConfirm();
      setOpen(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setPending(false);
    }
  };

  return (
    <AlertDialog
      open={open}
      onOpenChange={(next) => {
        if (pending) return;
        setOpen(next);
        if (next) setError(null);
      }}
    >
      <AlertDialogTrigger asChild>{trigger}</AlertDialogTrigger>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        {children}
        {error && <p style={{ color: 'var(--red)', font: '400 12px var(--font-mono)', margin: 0 }}>{error}</p>}
        <AlertDialogFooter>
          <AlertDialogCancel disabled={pending}>Cancel</AlertDialogCancel>
          <AlertDialogAction variant="destructive" disabled={pending} onClick={handleConfirm}>
            {pending ? <Spinner label={`${confirmLabel}…`} /> : confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
