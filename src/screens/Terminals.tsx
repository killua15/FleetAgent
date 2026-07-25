import { useEffect, useRef, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { AppTopBar } from '../components/AppTopBar';
import { TerminalPane } from '../components/TerminalPane';
import { toastError } from '../components/ErrorOverlay';
import { Button } from '../components/ui/button';
import { useNavigation } from '../nav/NavigationProvider';
import { autoGridColumns } from '../lib/gridLayout';
import { PROVIDER_META, type Account, type AccountAuth } from '../types/account';
import type { Project } from '../types/session';
import type { InitCommand, SuggestedAccount, TerminalInfo, TerminalTarget } from '../types/terminal';
import styles from './Terminals.module.css';

/** Terminals opened past this point stop being supervisable at a glance —
 * same threshold and same reasoning as the agent grid's own warning. */
const WARN_THRESHOLD = 4;

/** The "no account" row. A plain shell is still worth one click — it's what
 * the terminals were before they knew about accounts — but it sits below the
 * real ones because picking an account is the point of this list. */
const PLAIN_SHELL = '__plain__';

export function Terminals() {
  const { current, navigate } = useNavigation();
  const openFor = current.params?.openFor as TerminalTarget | undefined;
  const openForAccountId = current.params?.accountId as string | undefined;

  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [auth, setAuth] = useState<Record<string, AccountAuth>>({});
  const [authLoading, setAuthLoading] = useState(false);
  // The picker is two steps: pick the account, then pick where it runs. `null`
  // means we're on step one. The account leads because that's the decision
  // that actually varies — which AI you're talking to — while the directory
  // is usually whichever project you were already thinking about.
  const [pickedAccountId, setPickedAccountId] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [gridColumns, setGridColumns] = useState<number | 'auto'>('auto');
  const [pickerOpen, setPickerOpen] = useState(false);
  const [multiOpen, setMultiOpen] = useState(false);
  const [opening, setOpening] = useState(false);
  // Navigating here with an `openFor` param must open exactly one terminal,
  // not one per render — and not another one if the user navigates back to
  // this screen later with the same param still in the history entry.
  const handledOpenForRef = useRef(false);

  // `restore_terminals` reopens whatever was open when Fleet last closed on the
  // first mount of this run (and resumes each conversation), and just returns
  // the live list on later mounts — so navigating away and back never
  // re-restores. 80x24 is the shell's initial size; each pane's first fit()
  // corrects it.
  const loadTerminals = () => invoke<TerminalInfo[]>('restore_terminals', { cols: 80, rows: 24 }).then(setTerminals);

  const loadProjects = () => invoke<Project[]>('list_projects').then(setProjects);

  const loadAccounts = () => invoke<Account[]>('list_accounts').then(setAccounts);

  useEffect(() => {
    Promise.all([loadTerminals(), loadProjects(), loadAccounts()]).catch((e) => toastError(String(e)));
  }, []);

  // The usage poller flips an account to `exhausted` when it runs out of quota;
  // reloading accounts re-evaluates every panel's out-of-quota banner.
  useEffect(() => {
    const unlisten = listen('terminal-accounts-updated', () => {
      loadAccounts().catch(() => {});
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  /** Opens the macOS folder dialog and turns the chosen directory into a
   * project (creating it, or reusing an existing row for that path). Returns
   * the project, or null if the user cancelled. Adds it to the recents list so
   * it shows up immediately in both pickers. */
  const exploreForProject = async (): Promise<Project | null> => {
    const picked = await open({ directory: true, multiple: false, title: 'Choose a project folder' });
    if (typeof picked !== 'string') return null;
    const project = await invoke<Project>('ensure_project_for_path', { path: picked });
    setProjects((prev) => (prev.some((p) => p.id === project.id) ? prev : [...prev, project]));
    return project;
  };

  /** Loads each account's login state once, lazily — every account picker
   * needs it (ready vs. will-log-in, and the exact command shown), but it
   * costs a real `claude auth status` per account, so it waits for the first
   * picker to open rather than running on mount. */
  const ensureAuthLoaded = () => {
    if (authLoading || Object.keys(auth).length > 0) return;
    setAuthLoading(true);
    invoke<AccountAuth[]>('list_account_auth')
      .then((list) => setAuth(Object.fromEntries(list.map((a) => [a.accountId, a]))))
      .catch((e) => toastError(String(e)))
      .finally(() => setAuthLoading(false));
  };

  const openTerminal = async (target: TerminalTarget, accountId?: string, initCommand?: InitCommand) => {
    setOpening(true);
    try {
      // 80x24 is the classic default the shell starts against; the pane's
      // first `fit()` replaces it with the real size a moment later.
      const info = await invoke<TerminalInfo>('open_terminal', {
        target,
        accountId: accountId ?? null,
        init: initCommand ?? null,
        // Fleet mints the session id itself for a normal open.
        sessionUuid: null,
        cols: 80,
        rows: 24,
      });
      setTerminals((prev) => [...prev, info]);
      setPickerOpen(false);
      setPickedAccountId(null);
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };

  // Opening from a session card: inherit that session's account so the shell
  // is authenticated as the agent working there, and start its CLI.
  useEffect(() => {
    if (!openFor || handledOpenForRef.current) return;
    handledOpenForRef.current = true;
    openTerminal(openFor, openForAccountId, openForAccountId ? 'start' : undefined);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [openFor]);

  const openPicker = () => {
    setMultiOpen(false);
    setPickerOpen((wasOpen) => !wasOpen);
    setPickedAccountId(null);
    if (!pickerOpen) ensureAuthLoaded();
  };

  const openMultiPicker = () => {
    setPickerOpen(false);
    setMultiOpen((wasOpen) => !wasOpen);
    if (!multiOpen) ensureAuthLoaded();
  };

  /** The init command an account needs: its CLI when authenticated, its login
   * when not — the latter being the only way in Fleet to recover an account
   * whose token expired. `undefined` for the plain-shell pseudo-account. */
  const initFor = (accountId: string): InitCommand | undefined => {
    if (accountId === PLAIN_SHELL) return undefined;
    return auth[accountId]?.loggedIn ? 'start' : 'login';
  };

  /** One click on a target finishes the job: open the shell in that directory
   * under the chosen account, running the right command. */
  const openForTarget = (target: TerminalTarget) => {
    if (pickedAccountId === null) return;
    openTerminal(target, pickedAccountId === PLAIN_SHELL ? undefined : pickedAccountId, initFor(pickedAccountId));
  };

  const exploreAndOpen = async () => {
    try {
      const project = await exploreForProject();
      if (project) openForTarget({ kind: 'project', id: project.id });
    } catch (e) {
      toastError(String(e));
    }
  };

  const closeTerminal = async (id: string) => {
    try {
      await invoke('close_terminal', { id });
      setTerminals((prev) => prev.filter((t) => t.id !== id));
    } catch (e) {
      toastError(String(e));
    }
  };

  const reopenWithAccount = async (terminal: TerminalInfo, accountId: string) => {
    setOpening(true);
    try {
      const info = await invoke<TerminalInfo>('reopen_terminal_with_account', { id: terminal.id, accountId, cols: 80, rows: 24 });
      // The backend closed the old one and opened the new; swap it in place so
      // the panel keeps its spot in the grid.
      setTerminals((prev) => prev.map((t) => (t.id === terminal.id ? info : t)));
    } catch (e) {
      toastError(String(e));
    } finally {
      setOpening(false);
    }
  };

  const exhaustedAccountIds = new Set(accounts.filter((a) => a.status === 'exhausted').map((a) => a.id));

  const panels = terminals.filter((t) => !collapsed[t.id]);
  const hidden = terminals.filter((t) => collapsed[t.id]);
  const effectiveGridColumns = gridColumns === 'auto' ? autoGridColumns(panels.length) : gridColumns;

  const pickedAccount = accounts.find((a) => a.id === pickedAccountId);
  const pickedAuth = pickedAccountId ? auth[pickedAccountId] : undefined;

  return (
    <div className={styles.screen}>
      <AppTopBar
        primaryAction={
          <div style={{ display: 'flex', gap: 8 }}>
            <Button size="sm" variant="outline" onClick={openMultiPicker} disabled={opening}>
              + Multi terminal
            </Button>
            <Button size="sm" onClick={openPicker} disabled={opening}>
              {opening ? '…' : '+ New terminal'}
            </Button>
          </div>
        }
      />

      {pickerOpen && pickedAccountId === null && (
        <div className={styles.picker}>
          <span className={styles.pickerLabel}>
            WHICH ACCOUNT{authLoading && <span className={styles.pickerChecking}> · checking logins…</span>}
          </span>
          <div className={styles.pickerGroup}>
            {accounts.map((a) => {
              const meta = PROVIDER_META[a.provider];
              const status = auth[a.id];
              const unsupported = status && !status.supported;
              return (
                <button
                  key={a.id}
                  className={styles.accountRow}
                  disabled={unsupported}
                  title={unsupported ? `Fleet can't run a ${meta.name} CLI as a specific account yet` : undefined}
                  onClick={() => setPickedAccountId(a.id)}
                >
                  <span className={styles.provBadge} style={{ background: meta.tintVar, color: meta.colorVar }}>
                    {meta.letter}
                  </span>
                  <span className={styles.accountMain}>
                    <span className={styles.accountTop}>
                      <span className={styles.pickerName}>{a.name}</span>
                      <span className={styles.pickerPath}>{meta.name}</span>
                      <span className={styles.spacer} />
                      {/* Until the check lands there's nothing honest to claim,
                          so the row says nothing rather than guessing "ready". */}
                      {!status && <span className={styles.statusChecking}>…</span>}
                      {status && status.supported && status.loggedIn && <span className={styles.statusReady}>✓ ready</span>}
                      {status && status.supported && !status.loggedIn && <span className={styles.statusNeedsLogin}>✗ will log in</span>}
                      {unsupported && <span className={styles.statusChecking}>not supported yet</span>}
                    </span>
                    {/* The literal line that gets typed into the shell. Built
                        by the same backend function that runs it, so this is a
                        promise the app can actually keep. */}
                    {status?.command && <code className={styles.accountCommand}>{status.command}</code>}
                  </span>
                </button>
              );
            })}
            {accounts.length === 0 && <span className={styles.pickerEmpty}>no provider accounts yet — add one below to run its CLI</span>}
            <button className={styles.accountRow} onClick={() => setPickedAccountId(PLAIN_SHELL)}>
              <span className={styles.provBadge} style={{ background: 'var(--panel2)', color: 'var(--dim)' }}>
                $
              </span>
              <span className={styles.pickerName}>Plain shell</span>
              <span className={styles.pickerPath}>no account, nothing started</span>
            </button>
            <button className={styles.exploreRow} onClick={() => navigate('add-account')}>
              <span>＋ Add a provider account…</span>
            </button>
          </div>
        </div>
      )}

      {pickerOpen && pickedAccountId !== null && (
        <div className={styles.picker}>
          <div className={styles.envRow}>
            <button className={styles.backLink} onClick={() => setPickedAccountId(null)}>
              ‹ accounts
            </button>
            <span className={styles.pickerLabel}>
              {pickedAccount ? `${pickedAccount.name} — WHERE SHOULD IT RUN?` : 'WHERE SHOULD IT RUN?'}
            </span>
            {pickedAuth?.command && <code className={styles.accountCommand}>{pickedAuth.command}</code>}
            <span className={styles.spacer} />
            {pickedAuth && !pickedAuth.loggedIn && (
              <span className={styles.envWarn}>this account isn't logged in — the terminal will run its login instead</span>
            )}
            {pickedAccount?.billing === 'key' && (
              // Not a detail to leave implicit: the key is readable by `env`
              // and by anything the user runs in this shell.
              <span className={styles.envWarn}>⚠ this account's API key will be visible in the shell's environment</span>
            )}
          </div>
          <div className={styles.pickerGroup}>
            <span className={styles.pickerGroupTitle}>RECENT PROJECTS</span>
            {projects.map((p) => (
              <button key={p.id} className={styles.pickerRow} onClick={() => openForTarget({ kind: 'project', id: p.id })}>
                <span className={styles.pickerName}>{p.name}</span>
                <span className={styles.pickerPath}>{p.path}</span>
              </button>
            ))}
            <button className={styles.exploreRow} onClick={exploreAndOpen}>
              <span>📁 Explore another folder…</span>
            </button>
          </div>
        </div>
      )}

      {multiOpen && (
        <MultiTerminalPicker
          projects={projects}
          accounts={accounts}
          auth={auth}
          authLoading={authLoading}
          onExplore={exploreForProject}
          onClose={() => setMultiOpen(false)}
          onOpenAll={async (rows) => {
            for (const row of rows) {
              // Serial, not parallel: several accounts sharing a config dir
              // must not race, and even distinct ones spawning at once would
              // stampede the machine. A failure in one is surfaced but doesn't
              // abort the rest.
              try {
                await openTerminal({ kind: 'project', id: row.projectId }, row.accountId === PLAIN_SHELL ? undefined : row.accountId, initFor(row.accountId));
              } catch (e) {
                toastError(String(e));
              }
            }
            setMultiOpen(false);
          }}
        />
      )}

      <div className={styles.body}>
        {hidden.length > 0 && (
          <div className={styles.rail}>
            <span className={styles.railLabel}>MINIMIZED — STILL RUNNING</span>
            {hidden.map((t) => (
              <button
                key={t.id}
                className={styles.pill}
                title={t.cwd}
                onClick={() => setCollapsed((prev) => ({ ...prev, [t.id]: false }))}
              >
                <span className={styles.pillName}>{t.title}</span>
                <span className={styles.pillRestore}>⤢</span>
              </button>
            ))}
          </div>
        )}

        <div className={styles.gridArea}>
          {panels.length >= WARN_THRESHOLD && (
            <div className={styles.warnBanner}>
              <span className={styles.warnDot} />
              <span>{panels.length} terminals at once — collapse one to the rail ▁ to give the rest more room</span>
              <span className={styles.spacer} />
              <label className={styles.colControl}>
                columns
                <select
                  value={String(gridColumns)}
                  onChange={(e) => setGridColumns(e.target.value === 'auto' ? 'auto' : Number(e.target.value))}
                >
                  <option value="auto">auto</option>
                  {[1, 2, 3, 4].map((n) => (
                    <option key={n} value={n}>
                      {n}
                    </option>
                  ))}
                </select>
              </label>
            </div>
          )}

          {panels.length === 0 ? (
            <div className={styles.empty}>
              <span>no terminals open</span>
              <span>use “+ New terminal” above to open a Claude Code shell in a project under one of your accounts</span>
            </div>
          ) : (
            <div className={styles.grid} style={{ gridTemplateColumns: `repeat(${effectiveGridColumns}, minmax(0, 1fr))` }}>
              {panels.map((t) => (
                <div key={t.id} className={styles.card}>
                  <div className={styles.cardHeader}>
                    <span className={styles.kindBadge} style={{ background: t.kind === 'session' ? 'var(--tealD)' : 'var(--accD)', color: t.kind === 'session' ? 'var(--teal)' : 'var(--acc)' }}>
                      {t.kind === 'session' ? '⑃' : '▣'}
                    </span>
                    <span className={styles.cardTitle}>{t.title}</span>
                    {t.accountName && (
                      <span className={styles.accountBadge} title={`Running with ${t.accountName}'s credentials`}>
                        {t.accountName}
                      </span>
                    )}
                    <span className={styles.cardPath} title={t.cwd}>
                      {t.cwd}
                    </span>
                    <span className={styles.spacer} />
                    <Button
                      variant="outline"
                      size="icon-sm"
                      title="Collapse to rail — the shell keeps running"
                      onClick={() => setCollapsed((prev) => ({ ...prev, [t.id]: true }))}
                    >
                      ▁
                    </Button>
                    <Button variant="ghost" size="icon" title="Close terminal — kills the shell" style={{ color: 'var(--dim)' }} onClick={() => closeTerminal(t.id)}>
                      ×
                    </Button>
                  </div>
                  {t.accountId && exhaustedAccountIds.has(t.accountId) && (
                    <OutOfQuotaBanner terminal={t} busy={opening} onReopen={(accountId) => reopenWithAccount(t, accountId)} />
                  )}
                  {/* Keyed by id so React never recycles one pane's xterm
                      instance for a different terminal. */}
                  <TerminalPane terminalId={t.id} className={styles.term} />
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** Shown on a panel whose account has run out of quota. Asks the backend which
 * allowed account to offer, and reopens the terminal there on click. For a
 * claude→claude switch the conversation is carried over (Fleet copies the
 * transcript into the new account and resumes it), so it keeps going rather
 * than starting fresh. */
function OutOfQuotaBanner({ terminal, busy, onReopen }: { terminal: TerminalInfo; busy: boolean; onReopen: (accountId: string) => void }) {
  const [suggestion, setSuggestion] = useState<SuggestedAccount | null>(null);
  const [loaded, setLoaded] = useState(false);

  useEffect(() => {
    invoke<SuggestedAccount | null>('suggest_terminal_account', {
      kind: terminal.kind,
      sourceId: terminal.sourceId,
      exhaustedAccountId: terminal.accountId,
    })
      .then(setSuggestion)
      .catch(() => setSuggestion(null))
      .finally(() => setLoaded(true));
  }, [terminal.kind, terminal.sourceId, terminal.accountId]);

  // Only a terminal Fleet started with a session id can carry its conversation.
  const canKeepConversation = terminal.sessionUuid != null;

  return (
    <div className={styles.quotaBanner}>
      <span className={styles.quotaDot} />
      <span className={styles.quotaText}>
        {terminal.accountName ?? 'This account'} is out of quota.
        {suggestion
          ? canKeepConversation
            ? ' Reopening keeps the conversation.'
            : ' Reopening starts a fresh conversation.'
          : loaded
            ? ' No other allowed account to switch to.'
            : ''}
      </span>
      <span className={styles.spacer} />
      {suggestion && (
        <Button size="sm" disabled={busy} onClick={() => onReopen(suggestion.accountId)}>
          {busy ? '…' : `Reopen in ${suggestion.accountName}`}
        </Button>
      )}
    </div>
  );
}

interface MultiRow {
  id: string;
  projectId: string;
  accountId: string;
}

const newMultiRow = (): MultiRow => ({ id: crypto.randomUUID(), projectId: '', accountId: '' });

/** "Multi terminal": a list of (project, account) rows opened all at once —
 * the terminal-first replacement for the old multi-session flow. Deliberately
 * spare next to it: no prompt, model, or mode, because a real interactive
 * Claude Code session in the shell owns all of that itself. */
function MultiTerminalPicker({
  projects,
  accounts,
  auth,
  authLoading,
  onExplore,
  onOpenAll,
  onClose,
}: {
  projects: Project[];
  accounts: Account[];
  auth: Record<string, AccountAuth>;
  authLoading: boolean;
  onExplore: () => Promise<Project | null>;
  onOpenAll: (rows: MultiRow[]) => Promise<void>;
  onClose: () => void;
}) {
  const [rows, setRows] = useState<MultiRow[]>([newMultiRow()]);
  const [busy, setBusy] = useState(false);

  const update = (id: string, patch: Partial<MultiRow>) => setRows((prev) => prev.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  const removeRow = (id: string) => setRows((prev) => (prev.length > 1 ? prev.filter((r) => r.id !== id) : prev));

  const explore = async (rowId: string) => {
    try {
      const project = await onExplore();
      if (project) update(rowId, { projectId: project.id });
    } catch {
      // surfaced by the parent's toast path
    }
  };

  const ready = rows.filter((r) => r.projectId && r.accountId);

  const openAll = async () => {
    if (ready.length === 0 || busy) return;
    setBusy(true);
    try {
      await onOpenAll(ready);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.picker}>
      <span className={styles.pickerLabel}>
        OPEN SEVERAL AT ONCE{authLoading && <span className={styles.pickerChecking}> · checking logins…</span>}
      </span>
      <div className={styles.multiRows}>
        {rows.map((row) => {
          const status = row.accountId && row.accountId !== PLAIN_SHELL ? auth[row.accountId] : undefined;
          return (
            <div key={row.id} className={styles.multiRow}>
              <select className={styles.multiSelect} value={row.projectId} onChange={(e) => (e.target.value === '__explore__' ? explore(row.id) : update(row.id, { projectId: e.target.value }))}>
                <option value="">project…</option>
                {projects.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name}
                  </option>
                ))}
                <option value="__explore__">📁 explore a folder…</option>
              </select>
              <span className={styles.multiArrow}>→</span>
              <select className={styles.multiSelect} value={row.accountId} onChange={(e) => update(row.id, { accountId: e.target.value })}>
                <option value="">account…</option>
                {accounts.map((a) => (
                  <option key={a.id} value={a.id}>
                    {a.name} · {PROVIDER_META[a.provider].name}
                  </option>
                ))}
                <option value={PLAIN_SHELL}>Plain shell</option>
              </select>
              {status && !status.loggedIn && <span className={styles.statusNeedsLogin} title="will run its login">✗</span>}
              <button className={styles.multiRemove} title="remove row" disabled={rows.length === 1} onClick={() => removeRow(row.id)}>
                ×
              </button>
            </div>
          );
        })}
      </div>
      <div className={styles.multiActions}>
        <button className={styles.addRowLink} onClick={() => setRows((prev) => [...prev, newMultiRow()])}>
          + add row
        </button>
        <span className={styles.spacer} />
        <Button size="sm" variant="outline" onClick={onClose}>
          cancel
        </Button>
        <Button size="sm" disabled={ready.length === 0 || busy} onClick={openAll}>
          {busy ? 'opening…' : `Open ${ready.length || ''}`}
        </Button>
      </div>
    </div>
  );
}
