import { useState, type ReactNode } from 'react';
import { shouldRemoveConfigDirByDefault, type Account } from '../types/account';
import { ConfirmDeleteDialog } from './ConfirmDeleteDialog';
import { Checkbox } from './ui/checkbox';

/** `/Users/me/.claude-work` → `~/.claude-work`, for a line that has to fit. */
function abbreviateHome(path: string): string {
  return path.replace(/^\/Users\/[^/]+/, '~');
}

/**
 * Confirms deleting an account, and asks separately whether its config dir
 * goes too. That directory is the provider's login and the CLI's own history:
 * one Hangar created is safe to erase, but an adopted `~/.claude-work` is a
 * login the user may still use outside Hangar, so it's kept unless ticked.
 */
export function DeleteAccountDialog({
  trigger,
  account,
  onConfirm,
}: {
  trigger: ReactNode;
  account: Account;
  onConfirm: (removeConfigDir: boolean) => void | Promise<void>;
}) {
  const [removeDir, setRemoveDir] = useState(() => shouldRemoveConfigDirByDefault(account));
  const dir = abbreviateHome(account.configDirPath);

  return (
    <ConfirmDeleteDialog
      trigger={trigger}
      title={`Delete ${account.name}?`}
      description="This removes the account, its API key and its sessions from Hangar. This cannot be undone."
      onConfirm={() => onConfirm(removeDir)}
    >
      <label style={{ display: 'flex', alignItems: 'flex-start', gap: 8, font: '400 12px var(--font-ui)', color: 'var(--txt)', cursor: 'pointer' }}>
        <Checkbox checked={removeDir} onCheckedChange={(checked) => setRemoveDir(checked === true)} style={{ marginTop: 2 }} />
        <span>
          Also sign out and delete <code style={{ font: '500 11px var(--font-mono)' }}>{dir}</code>
          <span style={{ display: 'block', color: 'var(--dim)', fontSize: 11 }}>
            {account.configDirOwned === false
              ? 'This directory was yours before Hangar used it — its login and history go with it.'
              : 'Its login and the CLI’s history for this account go with it.'}
          </span>
        </span>
      </label>
    </ConfirmDeleteDialog>
  );
}
