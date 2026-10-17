import { useState } from 'react';
import { Popover as PopoverPrimitive } from 'radix-ui';
import { invoke } from '../lib/invoke';
import { Spinner } from './Spinner';
import { providerMeta, type Account } from '../types/account';
import styles from './AccountPickerButton.module.css';

/** A button that asks which account to run something under before doing it.
 *
 * Exists because the work it starts has no account to inherit: reviewing a PR
 * someone else opened touches a project that may have never run a session here,
 * so there is no previous choice to reuse and Hangar shouldn't invent one.
 *
 * Accounts load when the popover opens — most PR cards are never acted on.
 */
export function AccountPickerButton({
  label,
  title,
  busy,
  disabled,
  busyLabel = 'Opening…',
  unavailable,
  onPick,
}: {
  label: string;
  title?: string;
  busy?: boolean;
  disabled?: boolean;
  busyLabel?: string;
  /** Why an account can't do this job, or null when it can. Shown greyed out
   * with the reason, rather than left out — an account that silently isn't
   * there reads as a bug. */
  unavailable?: (account: Account) => string | null;
  onPick: (accountId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [accounts, setAccounts] = useState<Account[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const onOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) return;
    setError(null);
    invoke<Account[]>('list_accounts')
      .then(setAccounts)
      .catch((e) => setError(String(e)));
  };

  return (
    <PopoverPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <PopoverPrimitive.Trigger asChild>
        <button className={styles.trigger} disabled={busy || disabled} title={title}>
          {busy ? busyLabel : label}
          <span className={styles.caret}>▾</span>
        </button>
      </PopoverPrimitive.Trigger>
      <PopoverPrimitive.Portal>
        <PopoverPrimitive.Content className={styles.panel} align="start" sideOffset={6} collisionPadding={8}>
          <span className={styles.label}>WITH WHICH ACCOUNT</span>
          {!accounts && !error && (
            <span className={styles.note}>
              <Spinner label="loading accounts…" />
            </span>
          )}
          {error && <span className={styles.error}>{error}</span>}
          {accounts?.map((a) => {
            const meta = providerMeta(a.provider);
            const benched = a.status !== 'healthy';
            const cannot = unavailable?.(a) ?? null;
            return (
              <button
                key={a.id}
                className={styles.row}
                disabled={benched || cannot !== null}
                title={cannot ?? (benched ? 'This account is out of quota — it comes back on its own after its window resets' : undefined)}
                onClick={() => {
                  setOpen(false);
                  onPick(a.id);
                }}
              >
                <span className={styles.provBadge} style={{ background: meta.tintVar, color: meta.colorVar }}>
                  {meta.letter}
                </span>
                <span className={styles.rowName}>{a.name}</span>
                <span className={styles.spacer} />
                {cannot !== null ? (
                  <span className={styles.tagBenched}>not supported</span>
                ) : (
                  benched && <span className={styles.tagBenched}>out of quota</span>
                )}
              </button>
            );
          })}
          {accounts?.length === 0 && <span className={styles.note}>No provider accounts yet — add one in Accounts first.</span>}
        </PopoverPrimitive.Content>
      </PopoverPrimitive.Portal>
    </PopoverPrimitive.Root>
  );
}
