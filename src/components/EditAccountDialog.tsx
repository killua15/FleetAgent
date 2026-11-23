import { useState, type ReactNode } from 'react';
import { invoke } from '../lib/invoke';
import { endpointIsValid, isInsecureRemoteEndpoint, type Account, type AccountUpdate, type Scope } from '../types/account';
import type { ApiKeyCheck } from '../types/login';
import { EndpointFields, type EndpointValue } from './EndpointFields';
import { Button } from './ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, DialogTrigger } from './ui/dialog';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
import styles from '../screens/AddAccount.module.css';

interface EditAccountDialogProps {
  trigger: ReactNode;
  account: Account;
  /** Names of every *other* account, to refuse a duplicate before saving. */
  otherNames: string[];
  onSaved: (account: Account) => void;
}

/** What the form starts from, so reopening the dialog discards an unsaved edit. */
function initialEndpoint(account: Account): EndpointValue {
  const baseUrl = account.baseUrl ?? '';
  // A saved remote http address was already accepted once; asking again on
  // every edit would only teach people to tick boxes without reading them.
  return { baseUrl, defaultModel: account.defaultModel ?? '', allowInsecure: isInsecureRemoteEndpoint(baseUrl) };
}

/**
 * Edits an account in place — name, scope, spend cap, API key and, for a
 * custom endpoint, its address and model — so a changed server IP or a
 * rotated key no longer means deleting the account and its history.
 * Provider and billing aren't editable: changing either is a different account.
 */
export function EditAccountDialog({ trigger, account, otherNames, onSaved }: EditAccountDialogProps) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState(account.name);
  const [scope, setScope] = useState<Scope>(account.scope);
  const [monthlyCap, setMonthlyCap] = useState(account.monthlyCap === null ? '' : String(account.monthlyCap));
  const [secret, setSecret] = useState('');
  const [keyCheck, setKeyCheck] = useState<ApiKeyCheck | null>(null);
  const [checkingKey, setCheckingKey] = useState(false);
  const [endpoint, setEndpoint] = useState<EndpointValue>(() => initialEndpoint(account));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const isCustom = account.provider === 'custom';
  const isKey = account.billing === 'key';
  const trimmedName = name.trim();
  const duplicate = otherNames.some((n) => n.trim().toLowerCase() === trimmedName.toLowerCase());
  const capValue = monthlyCap.replace(/[^0-9.]/g, '');
  const capOk = capValue === '' || Number.isFinite(Number(capValue));
  const endpointOk = !isCustom || endpointIsValid(endpoint.baseUrl, endpoint.allowInsecure);
  const canSave = trimmedName !== '' && !duplicate && capOk && endpointOk && !saving;

  const reset = () => {
    setName(account.name);
    setScope(account.scope);
    setMonthlyCap(account.monthlyCap === null ? '' : String(account.monthlyCap));
    setSecret('');
    setKeyCheck(null);
    setEndpoint(initialEndpoint(account));
    setError(null);
  };

  const checkKey = async () => {
    setCheckingKey(true);
    try {
      setKeyCheck(await invoke<ApiKeyCheck>('validate_api_key', { provider: account.provider, apiKey: secret.trim() }));
    } catch (e) {
      setKeyCheck({ valid: false, message: String(e), unverifiable: true });
    } finally {
      setCheckingKey(false);
    }
  };

  const save = async () => {
    setSaving(true);
    setError(null);
    const update: AccountUpdate = {
      name: trimmedName,
      scope,
      ...(capValue === '' ? { clearMonthlyCap: true } : { monthlyCap: Number(capValue) }),
      ...(isKey && secret.trim() ? { secret: secret.trim() } : {}),
      ...(isCustom
        ? { baseUrl: endpoint.baseUrl.trim(), defaultModel: endpoint.defaultModel.trim(), allowInsecureHttp: endpoint.allowInsecure }
        : {}),
    };
    try {
      const saved = await invoke<Account>('update_account', { id: account.id, update });
      onSaved(saved);
      setOpen(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (next) reset();
        setOpen(next);
      }}
    >
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Edit {account.name}</DialogTitle>
          <DialogDescription>
            {account.planLabel}. Changes apply to new terminals — running ones keep the old settings until reopened.
          </DialogDescription>
        </DialogHeader>

        <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <div className={styles.field}>
            <Label className={styles.label}>NAME</Label>
            <Input className={styles.input} value={name} onChange={(e) => setName(e.target.value)} />
            {duplicate && (
              <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>
                An account named "{trimmedName}" already exists — pick a different name.
              </span>
            )}
          </div>
          <div className={styles.fieldRow}>
            <div className={styles.field}>
              <Label className={styles.label}>SCOPE</Label>
              <Select value={scope} onValueChange={(value) => setScope(value as Scope)}>
                <SelectTrigger className={styles.select}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="work">work</SelectItem>
                  <SelectItem value="personal">personal</SelectItem>
                </SelectContent>
              </Select>
            </div>
            <div className={styles.field}>
              <Label className={styles.label}>MONTHLY SPEND CAP</Label>
              <Input className={styles.input} value={monthlyCap} onChange={(e) => setMonthlyCap(e.target.value)} placeholder="no cap" />
            </div>
          </div>

          {isCustom && <EndpointFields value={endpoint} onChange={setEndpoint} />}

          {isKey && (
            <div className={styles.field}>
              <Label className={styles.label}>REPLACE API KEY</Label>
              <div style={{ display: 'flex', gap: 8 }}>
                <Input
                  className={styles.input}
                  style={{ flex: 1 }}
                  type="password"
                  value={secret}
                  onChange={(e) => {
                    setSecret(e.target.value);
                    setKeyCheck(null);
                  }}
                  placeholder="leave empty to keep the current key"
                />
                <Button variant="outline" disabled={!secret.trim() || checkingKey} onClick={checkKey}>
                  {checkingKey ? 'checking…' : 'check'}
                </Button>
              </div>
              {keyCheck && (
                <span
                  style={{
                    color: keyCheck.valid ? 'var(--grn)' : keyCheck.unverifiable ? 'var(--amber)' : 'var(--red)',
                    font: '500 11px var(--font-mono)',
                  }}
                >
                  {keyCheck.message}
                </span>
              )}
            </div>
          )}

          {error && <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>{error}</span>}
        </div>

        <DialogFooter>
          <Button variant="outline" disabled={saving} onClick={() => setOpen(false)}>
            Cancel
          </Button>
          <Button disabled={!canSave} onClick={save}>
            {saving ? 'saving…' : 'Save'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
