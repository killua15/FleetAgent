import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { SettingsTabs } from '../components/SettingsTabs';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import { Spinner } from '../components/Spinner';
import { Checkbox } from '../components/ui/checkbox';
import { Input } from '../components/ui/input';
import type { Project } from '../types/session';
import { endpointHost, type Account } from '../types/account';
import { ROTATION_POLICY_OPTIONS, type ProjectAccountRotation, type RotationPolicy } from '../types/settings';
import styles from './RotationSettings.module.css';

/** Rotation is per (project, provider), but only Claude has a turn runner
 * today — until a second provider ships, the screen edits the Claude policy
 * directly rather than showing a provider picker with one entry in it. */
const POLICY_PROVIDER = 'claude';

export function RotationSettings() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [rotation, setRotation] = useState<ProjectAccountRotation[]>([]);
  const [policy, setPolicy] = useState<RotationPolicy>('manual');
  const [loading, setLoading] = useState(false);
  /** Custom-endpoint accounts — the only kind that can be a fallback. */
  const [endpoints, setEndpoints] = useState<Account[]>([]);
  const [fallbackId, setFallbackId] = useState<string | null>(null);
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState('');

  const loadProjects = () =>
    invoke<Project[]>('list_projects').then((list) => {
      setProjects(list);
      setSelectedId((prev) => (prev && list.some((p) => p.id === prev) ? prev : (list[0]?.id ?? null)));
      return list;
    });

  useEffect(() => {
    loadProjects();
    invoke<Account[]>('list_accounts').then((list) => setEndpoints(list.filter((a) => a.provider === 'custom' && a.baseUrl)));
  }, []);

  const startRename = (p: Project) => {
    setRenamingId(p.id);
    setRenameValue(p.name);
  };

  const saveRename = async () => {
    if (!renamingId) return;
    const id = renamingId;
    const name = renameValue.trim();
    setRenamingId(null);
    if (!name) return;
    const updated = await invoke<Project>('update_project', { id, name });
    setProjects((prev) => prev.map((p) => (p.id === id ? updated : p)));
  };

  const handleDeleteProject = async (id: string) => {
    await invoke('delete_project', { id });
    await loadProjects();
  };

  useEffect(() => {
    if (!selectedId) return;
    setLoading(true);
    Promise.all([
      invoke<ProjectAccountRotation[]>('list_project_account_rotation', { projectId: selectedId }).then(setRotation),
      invoke<RotationPolicy>('get_rotation_policy', { projectId: selectedId, provider: POLICY_PROVIDER }).then(setPolicy),
      invoke<string | null>('get_project_fallback_account', { projectId: selectedId }).then(setFallbackId),
    ]).finally(() => setLoading(false));
  }, [selectedId]);

  const changePolicy = async (next: RotationPolicy) => {
    if (!selectedId) return;
    const previous = policy;
    setPolicy(next);
    try {
      await invoke('set_rotation_policy', { projectId: selectedId, provider: POLICY_PROVIDER, policy: next });
    } catch {
      setPolicy(previous);
    }
  };

  const changeFallback = async (next: string | null) => {
    if (!selectedId) return;
    const previous = fallbackId;
    setFallbackId(next);
    try {
      await invoke('set_project_fallback_account', { projectId: selectedId, accountId: next });
    } catch {
      setFallbackId(previous);
    }
  };

  const toggle = async (accountId: string, allowed: boolean) => {
    if (!selectedId) return;
    setRotation((prev) => prev.map((r) => (r.accountId === accountId ? { ...r, allowed } : r)));
    try {
      await invoke('set_project_account_allowed', { projectId: selectedId, accountId, allowed });
    } catch {
      // Revert on failure — best-effort optimistic update.
      setRotation((prev) => prev.map((r) => (r.accountId === accountId ? { ...r, allowed: !allowed } : r)));
    }
  };

  const allowedCount = rotation.filter((r) => r.allowed).length;
  const scopesInUse = new Set(rotation.filter((r) => r.allowed).map((r) => r.scope));
  const isMixed = scopesInUse.size > 1;
  const personalAccounts = rotation.filter((r) => r.scope === 'personal');
  const workAccounts = rotation.filter((r) => r.scope === 'work');
  const selected = projects.find((p) => p.id === selectedId);

  return (
    <div className={styles.screen}>
      <SettingsTabs
        active="rotation-settings"
        title="Account rotation"
        subtitle="which accounts each project may use. Opt-in only; nothing is shared by default."
      />
      <div className={styles.body}>
        <div className={styles.projectList}>
          <span className={styles.sectionLabel}>PROJECTS</span>
          {projects.map((p) => {
            const isSel = p.id === selectedId;
            return (
              <div key={p.id} className={isSel ? styles.projectRowActive : styles.projectRow}>
                <button className={styles.projectRowMain} onClick={() => setSelectedId(p.id)}>
                  <span className={styles.projectIcon}>{p.name[0]?.toUpperCase()}</span>
                  <span className={styles.projectInfo}>
                    {renamingId === p.id ? (
                      <Input
                        autoFocus
                        className={styles.renameInput}
                        value={renameValue}
                        onClick={(e) => e.stopPropagation()}
                        onChange={(e) => setRenameValue(e.target.value)}
                        onBlur={saveRename}
                        onKeyDown={(e) => e.key === 'Enter' && saveRename()}
                      />
                    ) : (
                      <span className={styles.projectName}>{p.name}</span>
                    )}
                  </span>
                </button>
                <button
                  className={styles.projectIconBtn}
                  title="Rename project"
                  onClick={(e) => {
                    e.stopPropagation();
                    startRename(p);
                  }}
                >
                  ✎
                </button>
                <ConfirmDeleteDialog
                  trigger={
                    <button className={styles.projectIconBtn} title="Delete project" onClick={(e) => e.stopPropagation()}>
                      🗑
                    </button>
                  }
                  title={`Delete ${p.name}?`}
                  description="This removes the project along with its sessions, worktrees, rotation settings, and memory files. This cannot be undone."
                  onConfirm={() => handleDeleteProject(p.id)}
                />
              </div>
            );
          })}
          <div className={styles.spacer} />
          <p className={styles.footnote}>
            Rotation is failover, not scheduling: the account you pick for a session is kept as long as it works, and Hangar only
            switches once it hits its rate limit.
          </p>
        </div>

        <div className={styles.checklist}>
          {selected && (
            <>
              <div className={styles.checklistHeader}>
                <span className={styles.checklistTitle}>{selected.name}</span>
                <span className={styles.dimText}>
                  {loading ? <Spinner label="loading…" /> : `${allowedCount} of ${rotation.length} accounts allowed`}
                </span>
              </div>
              <div className={styles.policyBox}>
                <span className={styles.policyLabel}>WHEN AN ACCOUNT HITS ITS LIMIT</span>
                <div className={styles.policyOptions}>
                  {ROTATION_POLICY_OPTIONS.map((opt) => (
                    <label key={opt.value} className={opt.value === policy ? styles.policyOptionActive : styles.policyOption}>
                      <input
                        type="radio"
                        name="rotation-policy"
                        checked={opt.value === policy}
                        onChange={() => changePolicy(opt.value)}
                      />
                      <span className={styles.policyOptionText}>
                        <span className={styles.policyOptionLabel}>{opt.label}</span>
                        <span className={styles.policyOptionHint}>{opt.hint}</span>
                      </span>
                    </label>
                  ))}
                </div>
              </div>

              <div className={styles.policyBox}>
                <span className={styles.policyLabel}>WHEN EVERY ACCOUNT IS OUT</span>
                {endpoints.length === 0 ? (
                  <span className={styles.policyOptionHint}>
                    No fallback available. Add a “Custom endpoint” account (OpenRouter, DeepSeek, a local gateway…) and it
                    can be offered here once this project’s Claude accounts run out.
                  </span>
                ) : (
                  <div className={styles.policyOptions}>
                    <label className={fallbackId === null ? styles.policyOptionActive : styles.policyOption}>
                      <input type="radio" name="fallback-account" checked={fallbackId === null} onChange={() => changeFallback(null)} />
                      <span className={styles.policyOptionText}>
                        <span className={styles.policyOptionLabel}>No fallback</span>
                        <span className={styles.policyOptionHint}>The terminal waits for an account to recover.</span>
                      </span>
                    </label>
                    {endpoints.map((a) => (
                      <label key={a.id} className={fallbackId === a.id ? styles.policyOptionActive : styles.policyOption}>
                        <input type="radio" name="fallback-account" checked={fallbackId === a.id} onChange={() => changeFallback(a.id)} />
                        <span className={styles.policyOptionText}>
                          <span className={styles.policyOptionLabel}>{a.name}</span>
                          <span className={styles.policyOptionHint}>
                            Offered on the out-of-quota banner, never switched to on its own. This project’s code goes to{' '}
                            {endpointHost(a.baseUrl ?? '')}.
                          </span>
                        </span>
                      </label>
                    ))}
                  </div>
                )}
              </div>

              <p className={styles.checklistHint}>Check the accounts this project is allowed to rotate across.</p>

              {allowedCount === 0 && policy !== 'manual' && (
                <div className={styles.mixWarning}>
                  <span>⚠</span>
                  <span>No accounts are checked, so there is nothing to switch to — sessions will still stop when their account runs out.</span>
                </div>
              )}

              {isMixed && (
                <div className={styles.mixWarning}>
                  <span>⚠</span>
                  <span>
                    This project mixes personal and work accounts. That's allowed only because you opted in here —
                    Hangar never does it on its own.
                  </span>
                </div>
              )}

              <AccountGroup title="PERSONAL ACCOUNTS" color="var(--teal)" accounts={personalAccounts} onToggle={toggle} />
              <AccountGroup title="WORK ACCOUNTS" color="var(--acc)" accounts={workAccounts} onToggle={toggle} />
            </>
          )}
          {!selected && <span className={styles.dimText}>No projects yet.</span>}
        </div>
      </div>
    </div>
  );
}

function AccountGroup({
  title,
  color,
  accounts,
  onToggle,
}: {
  title: string;
  color: string;
  accounts: ProjectAccountRotation[];
  onToggle: (accountId: string, allowed: boolean) => void;
}) {
  if (accounts.length === 0) return null;
  return (
    <div className={styles.group}>
      <div className={styles.groupHeader}>
        <span className={styles.groupTitle} style={{ color }}>
          {title}
        </span>
        <span className={styles.groupRule} />
      </div>
      <div className={styles.groupList}>
        {accounts.map((a) => (
          <label key={a.accountId} className={a.allowed ? styles.accountRowActive : styles.accountRow}>
            <Checkbox checked={a.allowed} onCheckedChange={(checked) => onToggle(a.accountId, checked === true)} />
            <span className={styles.accountBadge}>{a.provider[0].toUpperCase()}</span>
            <span className={styles.accountInfo}>
              <span className={styles.accountName}>{a.accountName}</span>
            </span>
          </label>
        ))}
      </div>
    </div>
  );
}
