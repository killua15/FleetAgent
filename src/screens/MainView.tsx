import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { AppTopBar } from '../components/AppTopBar';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import { ProjectPicker } from '../components/ProjectPicker';
import { Badge } from '../components/ui/badge';
import { Button } from '../components/ui/button';
import { Checkbox } from '../components/ui/checkbox';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Textarea } from '../components/ui/textarea';
import { useNavigation } from '../nav/NavigationProvider';
import { PROVIDER_META, type Account } from '../types/account';
import type {
  Effort,
  ModelOption,
  Project,
  Session,
  SessionBlock,
  SessionMode,
  SessionStatus,
  SessionView,
} from '../types/session';
import { EFFORT_OPTIONS, MODE_LABEL } from '../types/session';
import styles from './MainView.module.css';

const MODE_OPTIONS: SessionMode[] = ['plan', 'ask_edits', 'auto_edit', 'yolo'];

const STATUS_COLOR: Record<SessionStatus, string> = {
  running: 'var(--teal)',
  waiting: 'var(--dim)',
  reviewing: 'var(--amber)',
  done: 'var(--dim)',
};

interface OutputEventPayload {
  sessionId: string;
  kind: string;
  text: string;
}

/** 1-2 panels fit in a single row; from 3 on up, always split into exactly 2
 * rows as evenly as possible (4 → 2+2, 6 → 3+3, 5 → 3+2, ...) rather than
 * growing into a single ever-wider row or an unbounded number of rows. */
function autoGridColumns(panelCount: number): number {
  if (panelCount <= 0) return 1;
  const rows = panelCount <= 2 ? 1 : 2;
  return Math.ceil(panelCount / rows);
}

export function MainView() {
  const { navigate } = useNavigation();

  const [accounts, setAccounts] = useState<Account[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [sessions, setSessions] = useState<SessionView[]>([]);
  const [order, setOrder] = useState<string[]>([]);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [dragId, setDragId] = useState<string | null>(null);
  const [modules, setModules] = useState({ sidebar: true, usage: true, rail: true });
  const [modOpen, setModOpen] = useState(false);
  const [blocksBySession, setBlocksBySession] = useState<Record<string, SessionBlock[]>>({});
  const loadedBlocksRef = useRef<Set<string>>(new Set());
  const [deletingAccountId, setDeletingAccountId] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);
  const [stoppingSessionId, setStoppingSessionId] = useState<string | null>(null);
  const [gridColumns, setGridColumns] = useState<number | 'auto'>('auto');
  const [selectMode, setSelectMode] = useState(false);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  const loadAll = async () => {
    const [acc, proj, sess] = await Promise.all([
      invoke<Account[]>('list_accounts'),
      invoke<Project[]>('list_projects'),
      invoke<SessionView[]>('list_workspace_sessions'),
    ]);
    setAccounts(acc);
    setProjects(proj);
    setSessions(sess);
  };

  useEffect(() => {
    loadAll();
  }, []);

  const handleDeleteAccount = async (acc: Account) => {
    setDeleteError(null);
    setDeletingAccountId(acc.id);
    try {
      await invoke('delete_account', { id: acc.id });
      await loadAll();
    } catch (e) {
      setDeleteError(String(e));
    } finally {
      setDeletingAccountId(null);
    }
  };

  const handleDeleteSession = async (sessionId: string) => {
    try {
      await invoke('delete_session', { sessionId });
      await loadAll();
    } catch (e) {
      setDeleteError(String(e));
    }
  };

  const handleStopSession = async (sessionId: string) => {
    setStoppingSessionId(sessionId);
    try {
      await invoke('stop_session', { sessionId });
      setSessions((prev) => prev.map((s) => (s.id === sessionId ? { ...s, status: 'done' } : s)));
    } catch (e) {
      setDeleteError(String(e));
    } finally {
      setStoppingSessionId(null);
    }
  };

  useEffect(() => {
    const activeIds = sessions.filter((s) => s.status !== 'done').map((s) => s.id);
    setOrder((prev) => {
      const kept = prev.filter((id) => activeIds.includes(id));
      const added = activeIds.filter((id) => !kept.includes(id));
      return [...kept, ...added];
    });
  }, [sessions]);

  useEffect(() => {
    sessions.forEach((s) => {
      if (loadedBlocksRef.current.has(s.id)) return;
      loadedBlocksRef.current.add(s.id);
      invoke<SessionBlock[]>('list_session_blocks', { sessionId: s.id }).then((blocks) => {
        setBlocksBySession((prev) => ({ ...prev, [s.id]: blocks.slice(-30) }));
      });
    });
  }, [sessions]);

  useEffect(() => {
    const unlistenOutput = listen<OutputEventPayload>('session-output', (event) => {
      const { sessionId, kind, text } = event.payload;
      setBlocksBySession((prev) => {
        const list = prev[sessionId] ? [...prev[sessionId]] : [];
        list.push({
          id: `${sessionId}-${list.length}-${Date.now()}`,
          sessionId,
          kind: kind as SessionBlock['kind'],
          text,
          createdAt: new Date().toISOString(),
        });
        return { ...prev, [sessionId]: list.slice(-30) };
      });
    });
    const unlistenStatus = listen<[string, string]>('session-status', (event) => {
      const [sessionId, status] = event.payload;
      setSessions((prev) => prev.map((s) => (s.id === sessionId ? { ...s, status: status as SessionStatus } : s)));
    });
    const unlistenExhausted = listen('account-exhausted', () => {
      invoke<Account[]>('list_accounts').then(setAccounts);
    });

    return () => {
      unlistenOutput.then((f) => f());
      unlistenStatus.then((f) => f());
      unlistenExhausted.then((f) => f());
    };
  }, []);

  const visibleSessions = useMemo(() => sessions.filter((s) => s.status !== 'done'), [sessions]);
  const runningN = visibleSessions.filter((s) => s.status === 'running').length;
  const visibleIds = order.filter((id) => !collapsed[id]);
  const panels = visibleIds
    .map((id) => visibleSessions.find((s) => s.id === id))
    .filter((s): s is SessionView => Boolean(s));
  const collapsedIds = order.filter((id) => collapsed[id]);
  const effectiveGridColumns = gridColumns === 'auto' ? autoGridColumns(panels.length) : gridColumns;

  const toggleSelected = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const exitSelectMode = () => {
    setSelectMode(false);
    setSelectedIds(new Set());
  };

  const openSplitView = () => {
    navigate('split-view', { sessionIds: Array.from(selectedIds).slice(0, 4) });
    exitSelectMode();
  };

  const handleDrop = (targetId: string) => {
    if (!dragId || dragId === targetId) {
      setDragId(null);
      return;
    }
    setOrder((prev) => {
      const next = [...prev];
      const from = next.indexOf(dragId);
      const to = next.indexOf(targetId);
      if (from === -1 || to === -1) return prev;
      next.splice(from, 1);
      next.splice(to, 0, dragId);
      return next;
    });
    setDragId(null);
  };

  return (
    <div className={styles.screen}>
      <AppTopBar
        primaryAction={
          <>
            <Select value={String(gridColumns)} onValueChange={(v) => setGridColumns(v === 'auto' ? 'auto' : Number(v))}>
              <SelectTrigger size="sm" title="Grid columns — auto keeps panels to 2 rows max">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="auto">auto (2 rows)</SelectItem>
                {[1, 2, 3, 4].map((n) => (
                  <SelectItem key={n} value={String(n)}>
                    {n} col{n === 1 ? '' : 's'}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <ModulesControl
              modules={modules}
              open={modOpen}
              onToggleOpen={() => setModOpen((v) => !v)}
              onToggleModule={(key) => setModules((prev) => ({ ...prev, [key]: !prev[key] }))}
            />
            <Button
              variant="outline"
              style={selectMode ? { borderColor: 'var(--acc)', background: 'var(--accD)', color: 'var(--acc)' } : undefined}
              onClick={() => (selectMode ? exitSelectMode() : setSelectMode(true))}
            >
              {selectMode ? '✕ cancel' : '☑ select'}
            </Button>
            <NewSessionControl accounts={accounts} projects={projects} onCreated={loadAll} />
            <NewMultiSessionControl accounts={accounts} projects={projects} onCreated={loadAll} />
          </>
        }
      />
      <div className={styles.body}>
        {modules.sidebar && (
          <div className={styles.sidebar}>
            <div style={{ display: 'flex', alignItems: 'center', padding: '2px 8px 10px' }}>
              <span className={styles.sidebarLabel} style={{ padding: 0 }}>
                ACCOUNTS · SESSIONS
              </span>
              <span style={{ flex: 1 }} />
              <Button
                variant="ghost"
                size="sm"
                onClick={() => navigate('account-list')}
                style={{ color: 'var(--dim)', font: '500 9.5px var(--font-mono)' }}
                title="Manage accounts"
              >
                manage →
              </Button>
            </div>
            {deleteError && (
              <div style={{ color: 'var(--red)', font: '400 10px var(--font-mono)', padding: '0 8px 8px' }}>
                {deleteError}
              </div>
            )}
            {accounts.map((acc) => {
              const meta = PROVIDER_META[acc.provider];
              const accSessions = visibleSessions.filter((s) => s.accountId === acc.id);
              return (
                <div className={styles.accountGroup} key={acc.id}>
                  <div className={styles.accountRow}>
                    <Badge className={styles.accountBadge} style={{ background: meta.tintVar, color: meta.colorVar }}>
                      {meta.letter}
                    </Badge>
                    <span className={styles.accountInfo}>
                      <span className={styles.accountName}>{acc.name}</span>
                      <span className={styles.accountPlan}>{acc.planLabel}</span>
                    </span>
                    <span
                      className={styles.scopePill}
                      style={{ color: acc.scope === 'work' ? 'var(--acc)' : 'var(--teal)' }}
                    >
                      {acc.scope}
                    </span>
                    <ConfirmDeleteDialog
                      trigger={
                        <Button
                          variant="ghost"
                          size="icon"
                          title="Delete account"
                          disabled={deletingAccountId === acc.id}
                          style={{ color: 'var(--dim)', flex: 'none' }}
                        >
                          {deletingAccountId === acc.id ? '…' : '×'}
                        </Button>
                      }
                      title={`Delete ${acc.name}?`}
                      description="This removes the account and its credentials from Fleet. This cannot be undone."
                      onConfirm={() => handleDeleteAccount(acc)}
                    />
                  </div>
                  {modules.usage && <div className={styles.usageRow}>usage tracking — coming in rotation phase</div>}
                  {accSessions.map((s) => (
                    <div
                      className={styles.sessionRow}
                      key={s.id}
                      onClick={() => navigate('agent-panel', { sessionId: s.id })}
                    >
                      <span className={styles.sessionDot} style={{ background: STATUS_COLOR[s.status] }} />
                      <span className={styles.sessionName}>{s.name || s.projectName}</span>
                      <span className={styles.sessionProject}>{s.projectName}</span>
                    </div>
                  ))}
                </div>
              );
            })}
            <Button variant="outline" className={styles.addAccountLink} onClick={() => navigate('add-account')}>
              + add account
            </Button>
          </div>
        )}

        {modules.rail && (
          <div className={styles.rail}>
            <span className={styles.railLabel}>RAIL</span>
            {collapsedIds.map((id) => {
              const s = visibleSessions.find((x) => x.id === id);
              if (!s) return null;
              const initials = (s.name || s.projectName).slice(0, 2).toUpperCase();
              return (
                <Button
                  key={id}
                  variant="outline"
                  className={styles.railPill}
                  title={`${s.name || s.projectName} — click to restore`}
                  onClick={() => setCollapsed((prev) => ({ ...prev, [id]: false }))}
                >
                  <span className={styles.sessionDot} style={{ background: STATUS_COLOR[s.status] }} />
                  <span style={{ font: "700 9px var(--font-mono)", color: 'var(--mut)' }}>{initials}</span>
                </Button>
              );
            })}
            {collapsedIds.length === 0 && <span className={styles.railEmpty}>no minimized panels</span>}
          </div>
        )}

        <div className={styles.gridArea}>
          {runningN > 3 && (
            <div className={styles.warnBanner}>
              <span className={styles.warnDot} />
              <span className={styles.warnLabel}>{runningN} agents running at once — too many to supervise well</span>
              <span className={styles.spacer} />
              <span className={styles.warnTip}>tip: collapse one to the rail ▁</span>
            </div>
          )}
          {selectMode && selectedIds.size > 0 && (
            <div className={styles.warnBanner} style={{ background: 'var(--accD)', borderColor: 'var(--acc)' }}>
              <span style={{ font: '500 11.5px var(--font-ui)', color: 'var(--acc)' }}>
                {selectedIds.size} session{selectedIds.size === 1 ? '' : 's'} selected
              </span>
              <span className={styles.spacer} />
              <Button size="sm" onClick={openSplitView}>
                Open split view ({Math.min(selectedIds.size, 4)})
              </Button>
              <Button size="sm" variant="ghost" onClick={exitSelectMode}>
                cancel
              </Button>
            </div>
          )}
          {panels.length === 0 ? (
            <div className={styles.emptyGrid}>
              <span>no active sessions</span>
              <span>use “+ New session” above to start one</span>
            </div>
          ) : (
            <div className={styles.grid} style={{ gridTemplateColumns: `repeat(${effectiveGridColumns}, minmax(0, 1fr))` }}>
              {panels.map((s) => (
                <SessionPanelCard
                  key={s.id}
                  session={s}
                  blocks={blocksBySession[s.id] ?? []}
                  dragging={dragId === s.id}
                  onDragStart={() => setDragId(s.id)}
                  onDragOver={(e) => e.preventDefault()}
                  onDrop={() => handleDrop(s.id)}
                  onCollapse={() => setCollapsed((prev) => ({ ...prev, [s.id]: true }))}
                  onOpen={() => navigate('agent-panel', { sessionId: s.id })}
                  onConfirmDelete={() => handleDeleteSession(s.id)}
                  onStop={() => handleStopSession(s.id)}
                  stopping={stoppingSessionId === s.id}
                  selectMode={selectMode}
                  selected={selectedIds.has(s.id)}
                  onToggleSelect={() => toggleSelected(s.id)}
                />
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function SessionPanelCard({
  session,
  blocks,
  dragging,
  onDragStart,
  onDragOver,
  onDrop,
  onCollapse,
  onOpen,
  onConfirmDelete,
  onStop,
  stopping,
  selectMode,
  selected,
  onToggleSelect,
}: {
  session: SessionView;
  blocks: SessionBlock[];
  dragging: boolean;
  onDragStart: () => void;
  onDragOver: (e: React.DragEvent) => void;
  onDrop: () => void;
  onStop: () => void;
  stopping: boolean;
  onCollapse: () => void;
  onOpen: () => void;
  onConfirmDelete: () => void;
  selectMode: boolean;
  selected: boolean;
  onToggleSelect: () => void;
}) {
  const meta = PROVIDER_META[session.provider];
  const lastBlocks = blocks.slice(-4);
  const lastActivity = blocks.length > 0 ? blocks[blocks.length - 1].text : 'waiting for input…';
  const isRunning = session.status === 'running';

  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);

  const handleSendInline = async () => {
    if (!draft.trim() || sending || isRunning) return;
    const value = draft.trim();
    setDraft('');
    setSending(true);
    setSendError(null);
    try {
      await invoke('send_message', { sessionId: session.id, text: value });
    } catch (e) {
      setSendError(String(e));
    } finally {
      setSending(false);
    }
  };

  return (
    <div
      draggable={!selectMode}
      onDragStart={onDragStart}
      onDragOver={onDragOver}
      onDrop={onDrop}
      className={`${styles.card} ${dragging ? styles.cardDragging : ''}`}
    >
      <div className={styles.cardHeader}>
        {selectMode ? (
          <Checkbox checked={selected} onCheckedChange={onToggleSelect} style={{ marginRight: 2 }} />
        ) : (
          <span className={styles.grip}>⠿</span>
        )}
        <Badge className={styles.provBadgeSm} style={{ background: meta.tintVar, color: meta.colorVar }}>
          {meta.letter}
        </Badge>
        <Button variant="ghost" className={styles.cardTitle} onClick={selectMode ? onToggleSelect : onOpen} title="Open panel detail">
          {session.name || session.projectName}
        </Button>
        <span className={styles.cardProject}>{session.projectName}</span>
        <span className={styles.spacer} />
        {session.status === 'running' && (
          <Button
            variant="destructive"
            size="icon-sm"
            onClick={onStop}
            disabled={stopping}
            title="Stop this turn — recovers a stuck session without deleting it"
          >
            {stopping ? '…' : '■'}
          </Button>
        )}
        <span className={styles.statusPill} style={{ background: 'var(--tealD)', color: STATUS_COLOR[session.status] }}>
          {session.status}
        </span>
        <ConfirmDeleteDialog
          trigger={
            <Button variant="ghost" size="icon" title="Remove session" style={{ color: 'var(--dim)' }}>
              ×
            </Button>
          }
          title="Delete this session?"
          description="This removes the session and its full transcript. This cannot be undone."
          onConfirm={onConfirmDelete}
        />
        <Button variant="outline" size="icon-sm" className={styles.collapseBtn} title="Collapse to rail — keeps running" onClick={onCollapse}>
          ▁
        </Button>
      </div>
      <div className={styles.cardBody}>
        {lastBlocks.length === 0 && <div className={styles.blockLine}>no output yet</div>}
        {lastBlocks.map((b) => (
          <div key={b.id} className={`${styles.blockLine} ${b.kind === 'cmd' ? styles.blockLineCmd : ''}`}>
            {b.text}
          </div>
        ))}
      </div>
      {!selectMode && (
        <div style={{ display: 'flex', gap: 6, padding: '0 11px 8px', alignItems: 'center' }}>
          <Input
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            disabled={isRunning || sending}
            placeholder={isRunning ? 'agent is working…' : 'quick message…'}
            style={{ height: 26, fontSize: 11 }}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                e.preventDefault();
                handleSendInline();
              }
            }}
          />
          <Button size="icon-sm" disabled={isRunning || sending || !draft.trim()} onClick={handleSendInline}>
            ↑
          </Button>
        </div>
      )}
      {sendError && (
        <div style={{ padding: '0 11px 6px', font: '400 9.5px var(--font-mono)', color: 'var(--red)' }}>{sendError}</div>
      )}
      <div className={styles.cardFooter}>
        <span className={styles.actDot} />
        <span className={styles.activity}>{lastActivity}</span>
        <span className={styles.meta}>{session.model}</span>
      </div>
    </div>
  );
}

type ModuleKey = 'sidebar' | 'usage' | 'rail';

function ModulesControl({
  modules,
  open,
  onToggleOpen,
  onToggleModule,
}: {
  modules: Record<ModuleKey, boolean>;
  open: boolean;
  onToggleOpen: () => void;
  onToggleModule: (key: ModuleKey) => void;
}) {
  const rows: { key: ModuleKey; name: string; tag: string }[] = [
    { key: 'sidebar', name: 'Accounts sidebar', tag: 'Sidebar' },
    { key: 'usage', name: 'Usage meters', tag: 'UsageBars' },
    { key: 'rail', name: 'Pill rail', tag: 'PillRail' },
  ];

  return (
    <div className={styles.modulesWrap}>
      <Button variant="outline" className={`${styles.modulesBtn} ${open ? styles.on : ''}`} onClick={onToggleOpen}>
        ⬒ modules
      </Button>
      {open && (
        <div className={styles.modulesPanel}>
          <div className={styles.modulesTitle}>UI MODULES — MOUNT / UNMOUNT</div>
          {rows.map((r) => (
            <Label className={styles.moduleRow} key={r.key}>
              <Checkbox
                className={styles.moduleCheckbox}
                checked={modules[r.key]}
                onCheckedChange={() => onToggleModule(r.key)}
              />
              <span className={styles.moduleName}>{r.name}</span>
              <span className={styles.moduleTag}>{r.tag}</span>
            </Label>
          ))}
          <div className={styles.modulesNote}>Each region is a component — unmount what you don't need.</div>
        </div>
      )}
    </div>
  );
}

function NewSessionControl({
  accounts,
  projects,
  onCreated,
}: {
  accounts: Account[];
  projects: Project[];
  onCreated: () => Promise<void>;
}) {
  const { navigate } = useNavigation();
  const [open, setOpen] = useState(false);
  const [sessionName, setSessionName] = useState('');
  const [projectId, setProjectId] = useState('__new__');
  const [newProjectName, setNewProjectName] = useState('');
  const [newProjectPath, setNewProjectPath] = useState('');
  const [accountId, setAccountId] = useState('');
  const [models, setModels] = useState<ModelOption[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [model, setModel] = useState('');
  const [effort, setEffort] = useState<Effort>('high');
  const [mode, setMode] = useState<SessionMode>('ask_edits');
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!accountId) {
      setModels([]);
      setModel('');
      return;
    }
    setLoadingModels(true);
    invoke<ModelOption[]>('list_provider_models', { accountId })
      .then((list) => {
        setModels(list);
        setModel((current) => (list.some((m) => m.id === current) ? current : (list[0]?.id ?? '')));
      })
      .catch(() => setModels([]))
      .finally(() => setLoadingModels(false));
  }, [accountId]);

  const canSubmit =
    accountId !== '' &&
    model !== '' &&
    sessionName.trim() !== '' &&
    (projectId !== '__new__' || (newProjectName.trim() !== '' && newProjectPath.trim() !== ''));

  const handleCreate = async () => {
    if (!canSubmit || creating) return;
    setCreating(true);
    setError(null);
    try {
      let resolvedProjectId = projectId;
      if (projectId === '__new__') {
        const project = await invoke<Project>('create_project', {
          project: { name: newProjectName.trim(), path: newProjectPath.trim() },
        });
        resolvedProjectId = project.id;
      }
      const session = await invoke<Session>('start_session', {
        input: { projectId: resolvedProjectId, accountId, name: sessionName.trim(), model, effort, mode },
      });
      setOpen(false);
      await onCreated();
      navigate('agent-panel', { sessionId: session.id });
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className={styles.newSessionWrap}>
      <Button
        onClick={() => setOpen((v) => !v)}
        style={{ font: "600 11.5px 'IBM Plex Sans', sans-serif", padding: '6px 13px', borderRadius: 6 }}
      >
        + New session
      </Button>
      {open && (
        <div className={styles.newSessionPanel}>
          <div className={styles.field}>
            <Label className={styles.label}>SESSION NAME</Label>
            <Input
              className={styles.textInput}
              value={sessionName}
              onChange={(e) => setSessionName(e.target.value)}
              placeholder="fix-billing-rounding"
            />
          </div>
          <ProjectPicker
            projects={projects}
            projectId={projectId}
            onProjectIdChange={setProjectId}
            newProjectName={newProjectName}
            onNewProjectNameChange={setNewProjectName}
            newProjectPath={newProjectPath}
            onNewProjectPathChange={setNewProjectPath}
          />
          <div className={styles.field}>
            <Label className={styles.label}>ACCOUNT</Label>
            <Select value={accountId} onValueChange={setAccountId}>
              <SelectTrigger className={styles.select}>
                <SelectValue placeholder="select an account…" />
              </SelectTrigger>
              <SelectContent>
                {accounts.map((a) => (
                  <SelectItem key={a.id} value={a.id}>
                    {a.name} ({a.provider})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className={styles.field}>
            <Label className={styles.label}>MODEL</Label>
            <Select value={model} onValueChange={setModel} disabled={!accountId || loadingModels}>
              <SelectTrigger className={styles.select}>
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
          <div className={styles.field}>
            <Label className={styles.label}>EFFORT</Label>
            <Select value={effort} onValueChange={(value) => setEffort(value as Effort)}>
              <SelectTrigger className={styles.select}>
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
          <div className={styles.field}>
            <Label className={styles.label}>MODE</Label>
            <Select value={mode} onValueChange={(value) => setMode(value as SessionMode)}>
              <SelectTrigger className={styles.select}>
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
          {error && <span className={styles.errorText}>{error}</span>}
          <Button className={styles.createBtn} disabled={!canSubmit || creating} onClick={handleCreate}>
            {creating ? 'Starting…' : 'Start session'}
          </Button>
        </div>
      )}
    </div>
  );
}

const MAX_BULK_TARGETS = 6;

interface TargetState {
  id: string;
  accountId: string;
  model: string;
  effort: Effort;
  mode: SessionMode;
  name: string;
  useCustomPrompt: boolean;
  customPrompt: string;
}

function newTarget(): TargetState {
  return {
    id: crypto.randomUUID(),
    accountId: '',
    model: '',
    effort: 'high',
    mode: 'ask_edits',
    name: '',
    useCustomPrompt: false,
    customPrompt: '',
  };
}

function NewMultiSessionControl({
  accounts,
  projects,
  onCreated,
}: {
  accounts: Account[];
  projects: Project[];
  onCreated: () => Promise<void>;
}) {
  const { navigate } = useNavigation();
  const [open, setOpen] = useState(false);
  const [projectId, setProjectId] = useState('__new__');
  const [newProjectName, setNewProjectName] = useState('');
  const [newProjectPath, setNewProjectPath] = useState('');
  const [sharedPrompt, setSharedPrompt] = useState('');
  const [targets, setTargets] = useState<TargetState[]>([newTarget()]);
  const [creating, setCreating] = useState(false);
  const [errors, setErrors] = useState<string[]>([]);

  const updateTarget = (id: string, patch: Partial<TargetState>) => {
    setTargets((prev) => prev.map((t) => (t.id === id ? { ...t, ...patch } : t)));
  };

  const removeTarget = (id: string) => {
    setTargets((prev) => (prev.length > 1 ? prev.filter((t) => t.id !== id) : prev));
  };

  const canSubmit =
    sharedPrompt.trim() !== '' &&
    targets.length > 0 &&
    targets.every((t) => t.accountId !== '' && t.model !== '' && t.name.trim() !== '' && (!t.useCustomPrompt || t.customPrompt.trim() !== '')) &&
    (projectId !== '__new__' || (newProjectName.trim() !== '' && newProjectPath.trim() !== ''));

  const handleCreateAll = async () => {
    if (!canSubmit || creating) return;
    setCreating(true);
    setErrors([]);
    try {
      let resolvedProjectId = projectId;
      if (projectId === '__new__') {
        const project = await invoke<Project>('create_project', {
          project: { name: newProjectName.trim(), path: newProjectPath.trim() },
        });
        resolvedProjectId = project.id;
      }

      const results = await Promise.allSettled(
        targets.map(async (t) => {
          const session = await invoke<Session>('start_session', {
            input: { projectId: resolvedProjectId, accountId: t.accountId, name: t.name.trim(), model: t.model, effort: t.effort, mode: t.mode },
          });
          const prompt = (t.useCustomPrompt ? t.customPrompt : sharedPrompt).trim();
          await invoke('send_message', { sessionId: session.id, text: prompt });
          return session.id;
        }),
      );

      const createdIds: string[] = [];
      const failures: string[] = [];
      results.forEach((r, i) => {
        if (r.status === 'fulfilled') createdIds.push(r.value);
        else failures.push(`target ${i + 1}: ${String(r.reason)}`);
      });
      setErrors(failures);

      if (createdIds.length > 0) {
        setOpen(false);
        await onCreated();
        navigate('split-view', { sessionIds: createdIds.slice(0, 4) });
      }
    } finally {
      setCreating(false);
    }
  };

  return (
    <div className={styles.newSessionWrap}>
      <Button variant="outline" onClick={() => setOpen((v) => !v)}>
        + Multi-session
      </Button>
      {open && (
        <div className={styles.newSessionPanel} style={{ width: 420 }}>
          <ProjectPicker
            projects={projects}
            projectId={projectId}
            onProjectIdChange={setProjectId}
            newProjectName={newProjectName}
            onNewProjectNameChange={setNewProjectName}
            newProjectPath={newProjectPath}
            onNewProjectPathChange={setNewProjectPath}
          />
          <div className={styles.field}>
            <Label className={styles.label}>SHARED PROMPT</Label>
            <Textarea
              className={styles.textInput}
              rows={2}
              value={sharedPrompt}
              onChange={(e) => setSharedPrompt(e.target.value)}
              placeholder="sent to every target below, unless it customizes its own prompt"
            />
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
            {targets.map((t, i) => (
              <TargetRow
                key={t.id}
                index={i}
                target={t}
                accounts={accounts}
                canRemove={targets.length > 1}
                onChange={(patch) => updateTarget(t.id, patch)}
                onRemove={() => removeTarget(t.id)}
              />
            ))}
          </div>
          <Button
            variant="outline"
            disabled={targets.length >= MAX_BULK_TARGETS}
            onClick={() => setTargets((prev) => [...prev, newTarget()])}
          >
            + add target
          </Button>
          {errors.length > 0 && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 2 }}>
              {errors.map((e, i) => (
                <span key={i} className={styles.errorText}>
                  {e}
                </span>
              ))}
            </div>
          )}
          <Button className={styles.createBtn} disabled={!canSubmit || creating} onClick={handleCreateAll}>
            {creating ? 'Starting…' : `Start ${targets.length} session${targets.length === 1 ? '' : 's'}`}
          </Button>
        </div>
      )}
    </div>
  );
}

function TargetRow({
  index,
  target,
  accounts,
  canRemove,
  onChange,
  onRemove,
}: {
  index: number;
  target: TargetState;
  accounts: Account[];
  canRemove: boolean;
  onChange: (patch: Partial<TargetState>) => void;
  onRemove: () => void;
}) {
  const [models, setModels] = useState<ModelOption[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);

  useEffect(() => {
    if (!target.accountId) {
      setModels([]);
      return;
    }
    setLoadingModels(true);
    invoke<ModelOption[]>('list_provider_models', { accountId: target.accountId })
      .then((list) => {
        setModels(list);
        onChange({ model: list.some((m) => m.id === target.model) ? target.model : (list[0]?.id ?? '') });
      })
      .catch(() => setModels([]))
      .finally(() => setLoadingModels(false));
    // Only re-fetch when the account changes, not on every target field update.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target.accountId]);

  return (
    <div style={{ border: '1px solid var(--line)', borderRadius: 7, padding: 8, display: 'flex', flexDirection: 'column', gap: 6 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        <span className={styles.label} style={{ flex: 1 }}>
          TARGET {index + 1}
        </span>
        <Button variant="ghost" size="icon-sm" disabled={!canRemove} onClick={onRemove} style={{ color: 'var(--dim)' }}>
          ×
        </Button>
      </div>
      <Input
        className={styles.textInput}
        value={target.name}
        onChange={(e) => onChange({ name: e.target.value })}
        placeholder="session name"
      />
      <Select value={target.accountId} onValueChange={(value) => onChange({ accountId: value })}>
        <SelectTrigger className={styles.select}>
          <SelectValue placeholder="select an account…" />
        </SelectTrigger>
        <SelectContent>
          {accounts.map((a) => (
            <SelectItem key={a.id} value={a.id}>
              {a.name} ({a.provider})
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      <Select value={target.model} onValueChange={(value) => onChange({ model: value })} disabled={!target.accountId || loadingModels}>
        <SelectTrigger className={styles.select}>
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
      <div style={{ display: 'flex', gap: 6 }}>
        <Select value={target.effort} onValueChange={(value) => onChange({ effort: value as Effort })}>
          <SelectTrigger className={styles.select} style={{ flex: 1 }}>
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
        <Select value={target.mode} onValueChange={(value) => onChange({ mode: value as SessionMode })}>
          <SelectTrigger className={styles.select} style={{ flex: 1 }}>
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
      <Label className={styles.moduleRow} style={{ padding: '2px 0' }}>
        <Checkbox checked={target.useCustomPrompt} onCheckedChange={(checked) => onChange({ useCustomPrompt: checked === true })} />
        <span className={styles.moduleName} style={{ fontSize: 10.5 }}>
          customize prompt for this target
        </span>
      </Label>
      {target.useCustomPrompt && (
        <Textarea
          className={styles.textInput}
          rows={2}
          value={target.customPrompt}
          onChange={(e) => onChange({ customPrompt: e.target.value })}
          placeholder="prompt just for this target"
        />
      )}
    </div>
  );
}
