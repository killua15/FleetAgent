import { useState } from 'react';
import { Popover as PopoverPrimitive } from 'radix-ui';
import { invoke } from '../lib/invoke';
import { Spinner } from './Spinner';
import type { AccountAuth } from '../types/account';
import type { RotatableAccount, TerminalInfo } from '../types/terminal';
import styles from './AccountSwitcher.module.css';

/** The account chip in a terminal panel's header, as a switcher.
 *
 * The pool is the project's own: the accounts checked in Settings → Rotation
 * for this provider, which is what makes this a rotation control and not a
 * generic account list. Picking one reopens the terminal there —
 * `reopen_terminal_with_account` copies the Claude Code transcript into the
 * destination account's config dir and resumes it, so the conversation carries
 * over. The footer says up front whether it will, because a terminal that never
 * conversed has nothing to carry and lands on a fresh session.
 *
 * The list loads when the popover opens, not with the panel: it costs a query
 * per terminal and a batch `claude auth status`, and most panels are never
 * switched.
 */
export function AccountSwitcher({
  terminal,
  auth,
  onEnsureAuth,
  onSwitch,
  switching,
  onManagePool,
}: {
  terminal: TerminalInfo;
  auth: Record<string, AccountAuth>;
  onEnsureAuth: () => void;
  onSwitch: (accountId: string) => void;
  switching: boolean;
  onManagePool: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [accounts, setAccounts] = useState<RotatableAccount[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const onOpenChange = (next: boolean) => {
    setOpen(next);
    if (!next) return;
    onEnsureAuth();
    // Re-read on every open rather than caching: an account can be benched by
    // the usage poller, or checked in Settings, while the panel sits there.
    setError(null);
    setAccounts(null);
    invoke<RotatableAccount[]>('list_terminal_rotation_accounts', {
      kind: terminal.kind,
      sourceId: terminal.sourceId,
      currentAccountId: terminal.accountId,
    })
      .then(setAccounts)
      .catch((e) => setError(String(e)));
  };

  const others = accounts?.filter((a) => !a.current) ?? [];

  return (
    <PopoverPrimitive.Root open={open} onOpenChange={onOpenChange}>
      <PopoverPrimitive.Trigger asChild>
        <button
          className={styles.chip}
          disabled={switching}
          title={`Running with ${terminal.accountName}'s credentials — click to switch account`}
        >
          <span className={styles.chipName}>{terminal.accountName}</span>
          <span className={styles.chipCaret}>{switching ? '…' : '▾'}</span>
        </button>
      </PopoverPrimitive.Trigger>
      <PopoverPrimitive.Portal>
        <PopoverPrimitive.Content className={styles.panel} align="start" sideOffset={6} collisionPadding={8}>
          <span className={styles.label}>SWITCH ACCOUNT</span>

          {!accounts && !error && (
            <span className={styles.note}>
              <Spinner label="loading accounts…" />
            </span>
          )}
          {error && <span className={styles.error}>{error}</span>}

          {accounts?.map((a) => {
            const status = auth[a.accountId];
            const benched = a.status !== 'healthy';
            return (
              <button
                key={a.accountId}
                className={a.current ? styles.rowCurrent : styles.row}
                disabled={a.current || benched || switching}
                title={benched ? 'This account is out of quota — it comes back on its own after its window resets' : undefined}
                onClick={() => {
                  setOpen(false);
                  onSwitch(a.accountId);
                }}
              >
                <span className={styles.scopeDot} style={{ background: a.scope === 'work' ? 'var(--acc)' : 'var(--teal)' }} />
                <span className={styles.rowName}>{a.accountName}</span>
                <span className={styles.spacer} />
                {a.current && <span className={styles.tagCurrent}>current</span>}
                {!a.current && a.suggested && <span className={styles.tagSuggested}>suggested</span>}
                {benched && <span className={styles.tagBenched}>out of quota</span>}
                {/* Login state is the same batch check the new-terminal picker
                    uses. An account that isn't logged in is still selectable —
                    the terminal lands on its login, which is Hangar's only way
                    to recover one. */}
                {!benched && !a.current && status && !status.loggedIn && <span className={styles.tagLogin}>will log in</span>}
              </button>
            );
          })}

          {accounts && others.length === 0 && (
            <span className={styles.note}>
              This project has no other account to rotate to. Check the ones it may use in Settings → Rotation.
            </span>
          )}

          {accounts && others.length > 0 && (
            <span className={styles.footer}>
              {terminal.sessionUuid
                ? 'Switching carries this conversation over — the agent picks up where it left off.'
                : 'This terminal has no conversation to carry, so switching starts a fresh one.'}
            </span>
          )}

          <button className={styles.manageLink} onClick={onManagePool}>
            Manage which accounts this project may use →
          </button>
        </PopoverPrimitive.Content>
      </PopoverPrimitive.Portal>
    </PopoverPrimitive.Root>
  );
}
