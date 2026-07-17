import { useEffect, useState, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { Account } from '../types/account';
import {
  EFFORT_OPTIONS,
  MODE_LABEL,
  MODE_OPTIONS,
  type Effort,
  type ModelOption,
  type NewSubsession,
  type Session,
  type SessionMode,
  type SessionView,
} from '../types/session';
import type { ProjectAccountRotation } from '../types/settings';
import { Button } from './ui/button';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from './ui/dialog';
import { Input } from './ui/input';
import { Label } from './ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
import { Textarea } from './ui/textarea';

const fieldLabel = { font: '600 9.5px var(--font-mono)', letterSpacing: '0.1em', color: 'var(--dim)' } as const;
const fieldStyle = { display: 'flex', flexDirection: 'column' as const, gap: 4 };

interface CreateSubsessionDialogProps {
  trigger: ReactNode;
  parentSession: SessionView;
  accounts: Account[];
  onCreated: (session: Session) => void;
}

/**
 * Branches an existing session into a subsession that works in its own git
 * worktree (or copied folder) in parallel — shared between AgentPanel's
 * header and each SessionPanelCard in the grid so both entry points create
 * subsessions identically.
 */
export function CreateSubsessionDialog({ trigger, parentSession, accounts, onCreated }: CreateSubsessionDialogProps) {
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const [accountId, setAccountId] = useState(parentSession.accountId);
  const [rotation, setRotation] = useState<ProjectAccountRotation[] | null>(null);
  const [models, setModels] = useState<ModelOption[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [model, setModel] = useState(parentSession.model);
  const [effort, setEffort] = useState<Effort>(parentSession.effort);
  const [mode, setMode] = useState<SessionMode>(parentSession.mode);
  const [setupCommand, setSetupCommand] = useState('');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    // Reset to the parent's own settings each time the dialog opens, so a
    // previous subsession's edits don't leak into the next one.
    setName('');
    setAccountId(parentSession.accountId);
    setModel(parentSession.model);
    setEffort(parentSession.effort);
    setMode(parentSession.mode);
    setSetupCommand('');
    setError(null);
  }, [open, parentSession.accountId, parentSession.model, parentSession.effort, parentSession.mode]);

  useEffect(() => {
    if (!open) return;
    invoke<ProjectAccountRotation[]>('list_project_account_rotation', { projectId: parentSession.projectId }).then(setRotation);
  }, [open, parentSession.projectId]);

  // The parent's own account is always offered even if rotation would
  // otherwise exclude it — a subsession naturally inherits it by default.
  const selectableAccounts = rotation
    ? accounts.filter((a) => a.id === parentSession.accountId || rotation.find((r) => r.accountId === a.id)?.allowed)
    : accounts;

  useEffect(() => {
    if (!accountId) {
      setModels([]);
      return;
    }
    setLoadingModels(true);
    invoke<ModelOption[]>('list_provider_models', { accountId })
      .then((list) => {
        setModels(list);
        setModel((current) => (list.some((m) => m.id === current) ? current : (list[0]?.id ?? current)));
      })
      .catch(() => setModels([]))
      .finally(() => setLoadingModels(false));
  }, [accountId]);

  const canSubmit = name.trim() !== '' && accountId !== '' && model !== '';

  const handleCreate = async () => {
    if (!canSubmit || creating) return;
    setCreating(true);
    setError(null);
    try {
      const input: NewSubsession = {
        parentSessionId: parentSession.id,
        name: name.trim(),
        accountId,
        model,
        effort,
        mode,
        setupCommand: setupCommand.trim() || undefined,
      };
      const session = await invoke<Session>('create_subsession', { input });
      setOpen(false);
      onCreated(session);
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>{trigger}</DialogTrigger>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Create subsession</DialogTitle>
          <DialogDescription>
            Branches off "{parentSession.name || parentSession.projectName}" into its own isolated worktree, so it
            can work in parallel without touching the same files.
          </DialogDescription>
        </DialogHeader>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <div style={fieldStyle}>
            <Label style={fieldLabel}>NAME</Label>
            <Input value={name} onChange={(e) => setName(e.target.value)} placeholder="try-alternate-approach" />
          </div>
          <div style={fieldStyle}>
            <Label style={fieldLabel}>ACCOUNT</Label>
            <Select value={accountId} onValueChange={setAccountId}>
              <SelectTrigger>
                <SelectValue placeholder="select an account…" />
              </SelectTrigger>
              <SelectContent>
                {selectableAccounts.map((a) => (
                  <SelectItem key={a.id} value={a.id}>
                    {a.name} ({a.provider})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div style={fieldStyle}>
            <Label style={fieldLabel}>MODEL</Label>
            <Select value={model} onValueChange={setModel} disabled={!accountId || loadingModels}>
              <SelectTrigger>
                <SelectValue placeholder={loadingModels ? 'loading…' : 'select an account first'} />
              </SelectTrigger>
              <SelectContent>
                {models.map((m) => (
                  <SelectItem key={m.id} value={m.id}>
                    {m.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div style={{ display: 'flex', gap: 8 }}>
            <div style={{ ...fieldStyle, flex: 1 }}>
              <Label style={fieldLabel}>EFFORT</Label>
              <Select value={effort} onValueChange={(v) => setEffort(v as Effort)}>
                <SelectTrigger style={{ width: '100%' }}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {EFFORT_OPTIONS.map((e) => (
                    <SelectItem key={e} value={e}>
                      {e}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div style={{ ...fieldStyle, flex: 1 }}>
              <Label style={fieldLabel}>MODE</Label>
              <Select value={mode} onValueChange={(v) => setMode(v as SessionMode)}>
                <SelectTrigger style={{ width: '100%' }}>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {MODE_OPTIONS.map((m) => (
                    <SelectItem key={m} value={m}>
                      {MODE_LABEL[m]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
          <div style={fieldStyle}>
            <Label style={fieldLabel}>SETUP COMMAND (OPTIONAL)</Label>
            <Textarea
              rows={2}
              value={setupCommand}
              onChange={(e) => setSetupCommand(e.target.value)}
              placeholder="npm install"
            />
          </div>
          {error && <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>{error}</span>}
        </div>
        <DialogFooter>
          <Button disabled={!canSubmit || creating} onClick={handleCreate}>
            {creating ? 'Creating…' : 'Create subsession'}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
