import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '../lib/invoke';
import { listen } from '@tauri-apps/api/event';
import { openUrl } from '@tauri-apps/plugin-opener';
import { Highlight, themes } from 'prism-react-renderer';
import { useNavigation } from '../nav/NavigationProvider';
import { useTheme } from '../theme/ThemeProvider';
import type { Account } from '../types/account';
import type { PrComment, PullRequestInfo, PullRequestPreview } from '../types/pullRequest';
import {
  EFFORT_OPTIONS,
  MODE_LABEL,
  type AccountUsageSummary,
  type BlockKind,
  type Effort,
  type FileEdit,
  type FileTreeNode,
  type ModelOption,
  type PermissionRequest,
  type ResumableConversation,
  type Session,
  type SessionBlock,
  type SessionMode,
  type SessionStatus,
  type SessionView,
  type TurnUsage,
} from '../types/session';
import type { AppSettings } from '../types/settings';
import { Button } from '../components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '../components/ui/dialog';
import { Input } from '../components/ui/input';
import { Label } from '../components/ui/label';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Textarea } from '../components/ui/textarea';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import { CreateSubsessionDialog } from '../components/CreateSubsessionDialog';
import { Spinner } from '../components/Spinner';
import { UsageBar } from '../components/UsageBar';
import { severityOfBlock, type Severity } from '../lib/blockSeverity';
import { diffLines, diffStats, type DiffLine } from '../lib/diffLines';
import { fileIcon } from '../lib/fileIcon';
import { FileTreeRows } from '../components/FileTree';
import { languageFor } from '../lib/language';
import { countSessionSubtree } from '../lib/sessionTree';
import {
  contextSeverity,
  contextSeverityColor,
  contextWindowFor,
  formatTokenCount,
  formatUsd,
  sessionTokenBudgetFor,
  weeklyTokenBudgetFor,
} from '../lib/tokenBudget';
import { groupBlocksIntoTurns } from '../lib/turns';
import { useRealUsage } from '../lib/useRealUsage';
import styles from './AgentPanel.module.css';

interface Attachment {
  id: string;
  name: string;
  previewUrl: string;
  savedPath: string | null;
}

const MODE_OPTIONS: SessionMode[] = ['plan', 'ask_edits', 'auto_edit', 'yolo'];

const FILES_VIEW_WIDTH_KEY = 'hangar.filesViewWidth';
const FILES_VIEW_WIDTH_DEFAULT = 280;
const FILES_VIEW_WIDTH_MIN = 260;
const FILES_VIEW_WIDTH_MAX = 900;

function readStoredFilesViewWidth(): number {
  const stored = Number(window.localStorage.getItem(FILES_VIEW_WIDTH_KEY));
  if (Number.isFinite(stored) && stored >= FILES_VIEW_WIDTH_MIN && stored <= FILES_VIEW_WIDTH_MAX) return stored;
  return FILES_VIEW_WIDTH_DEFAULT;
}

const TAG_STYLE: Record<BlockKind, { color: string; bg: string }> = {
  you: { color: 'var(--acc)', bg: 'var(--accD)' },
  think: { color: 'var(--dim)', bg: 'var(--panel2)' },
  edit: { color: 'var(--grn)', bg: 'rgba(95,211,154,.12)' },
  cmd: { color: 'var(--mut)', bg: 'var(--panel2)' },
  agent: { color: 'var(--teal)', bg: 'var(--tealD)' },
  out: { color: 'var(--mut)', bg: 'var(--panel2)' },
  sum: { color: 'var(--teal)', bg: 'var(--tealD)' },
  rev: { color: 'var(--acc)', bg: 'var(--accD)' },
};

const ERROR_TAG_STYLE = { color: 'var(--red)', bg: 'var(--redD)' };

/** Severity overrides kind-based coloring only for the `error` case — "color
 * = severity, not decoration": a `notable` (agent/sum) or `routine` block
 * keeps exactly the styling it already had, since those aren't exceptions. */
function wrapStyle(kind: BlockKind, severity: Severity): React.CSSProperties {
  const base: React.CSSProperties = { opacity: kind === 'think' ? 0.8 : 1, fontStyle: kind === 'think' ? 'italic' : 'normal' };
  if (severity === 'error') {
    return { ...base, background: 'var(--redD)', border: '1px solid var(--red)' };
  }
  if (kind === 'you') {
    return { ...base, background: 'var(--panel)', border: '1px solid var(--line)' };
  }
  const borderLeft = kind === 'agent' || kind === 'sum' ? '2px solid var(--teal)' : kind === 'edit' ? '2px solid var(--grn)' : '2px solid var(--line)';
  return { ...base, background: 'transparent', border: '1px solid transparent', borderLeft };
}

function textColor(kind: BlockKind, severity: Severity): string {
  if (severity === 'error') return 'var(--red)';
  if (kind === 'think') return 'var(--dim)';
  if (kind === 'you') return 'var(--txt)';
  return 'var(--mut)';
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function formatWorkingDuration(totalSeconds: number): string {
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const m = Math.floor(totalSeconds / 60);
  const s = totalSeconds % 60;
  return `${m}m ${s}s`;
}

/** Short, human-readable summary of a tool_input for the permission banner —
 * the field that actually matters differs per tool (a shell command for
 * Bash, a file path for Edit/Write/MultiEdit), so this isn't just a raw
 * JSON dump. Returns '' when there's nothing worth summarizing. */
function summarizePermissionInput(toolName: string, toolInput: unknown): string {
  if (!toolInput || typeof toolInput !== 'object') return '';
  const input = toolInput as Record<string, unknown>;
  if (toolName === 'Bash' && typeof input.command === 'string') return input.command;
  if (typeof input.file_path === 'string') return input.file_path;
  try {
    const json = JSON.stringify(input);
    return json.length > 160 ? `${json.slice(0, 160)}…` : json;
  } catch {
    return '';
  }
}

export function AgentPanel() {
  const { current, back } = useNavigation();
  const sessionId = current.params?.sessionId as string | undefined;

  if (!sessionId) {
    return (
      <div className={styles.screen}>
        <div className={styles.header}>
          <Button variant="outline" size="icon" className={styles.backBtn} onClick={back}>
            ←
          </Button>
          <span className={styles.title}>loading session…</span>
        </div>
      </div>
    );
  }

  return <AgentPanelBody sessionId={sessionId} onClose={back} />;
}

interface AgentPanelBodyProps {
  sessionId: string;
  onClose: () => void;
  closeIcon?: string;
  closeTitle?: string;
  /** Renders to fill its container (100%) instead of the full viewport — used inside SplitView's grid cells. */
  embedded?: boolean;
}

export function AgentPanelBody({
  sessionId,
  onClose,
  closeIcon = '←',
  closeTitle = 'Back to workspace grid',
  embedded = false,
}: AgentPanelBodyProps) {
  const { navigate } = useNavigation();
  const [session, setSession] = useState<SessionView | null>(null);
  const [blocks, setBlocks] = useState<SessionBlock[]>([]);
  const [models, setModels] = useState<ModelOption[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [allSessions, setAllSessions] = useState<SessionView[]>([]);
  const [showThinking, setShowThinking] = useState(true);
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [resumeOpen, setResumeOpen] = useState(false);
  const [resumeOptions, setResumeOptions] = useState<ResumableConversation[]>([]);
  const [loadingResume, setLoadingResume] = useState(false);
  const [resumingId, setResumingId] = useState<string | null>(null);
  const [appSettings, setAppSettings] = useState<AppSettings>({
    reviewerEnabled: false,
    reviewerAccountId: null,
    notifyNeedsApproval: false,
    notifySessionWaiting: false,
    notifyAccountExhausted: false,
    notifySpendCap: false,
    notifyPrComments: false,
    agentTreesEnabled: false,
    includeOwnPrComments: false,
  });
  const [reviewing, setReviewing] = useState(false);
  const [pendingApproval, setPendingApproval] = useState<PermissionRequest | null>(null);
  const [pullRequest, setPullRequest] = useState<PullRequestInfo | null>(null);
  const [creatingPr, setCreatingPr] = useState(false);
  const [prDialogOpen, setPrDialogOpen] = useState(false);
  const [prError, setPrError] = useState<string | null>(null);
  const [prPreview, setPrPreview] = useState<PullRequestPreview | null>(null);
  const [loadingPrPreview, setLoadingPrPreview] = useState(false);
  const [prTitle, setPrTitle] = useState('');
  const [newPrComments, setNewPrComments] = useState<PrComment[]>([]);
  const [checkingPrComments, setCheckingPrComments] = useState(false);
  const [sendingPrComments, setSendingPrComments] = useState(false);
  const [respondingApproval, setRespondingApproval] = useState(false);
  const pendingApprovalRef = useRef<PermissionRequest | null>(null);
  const [filesOpen, setFilesOpen] = useState(false);
  const [fileTree, setFileTree] = useState<FileTreeNode[]>([]);
  const [collapsedPaths, setCollapsedPaths] = useState<Set<string>>(new Set());
  const [selectedFilePath, setSelectedFilePath] = useState<string | null>(null);
  const [fileEdits, setFileEdits] = useState<FileEdit[]>([]);
  const [fileContent, setFileContent] = useState<string | null>(null);
  const [fileContentError, setFileContentError] = useState<string | null>(null);
  const [contentTab, setContentTab] = useState<'diff' | 'file'>('diff');
  const [allFileEdits, setAllFileEdits] = useState<FileEdit[]>([]);
  const [allTurnUsage, setAllTurnUsage] = useState<TurnUsage[]>([]);
  const [accountWeeklyUsage, setAccountWeeklyUsage] = useState<AccountUsageSummary | null>(null);
  const [collapsedTurns, setCollapsedTurns] = useState<Set<string>>(new Set());
  const [filesViewWidth, setFilesViewWidth] = useState(readStoredFilesViewWidth);
  const lastSelectedFileRef = useRef<string | null>(null);
  const timelineRef = useRef<HTMLDivElement>(null);

  const startFilesViewResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = filesViewWidth;
    document.body.style.userSelect = 'none';
    const handleMove = (moveEvent: MouseEvent) => {
      const next = startWidth - (moveEvent.clientX - startX);
      setFilesViewWidth(Math.min(FILES_VIEW_WIDTH_MAX, Math.max(FILES_VIEW_WIDTH_MIN, next)));
    };
    const handleUp = () => {
      document.body.style.userSelect = '';
      window.removeEventListener('mousemove', handleMove);
      window.removeEventListener('mouseup', handleUp);
      setFilesViewWidth((w) => {
        window.localStorage.setItem(FILES_VIEW_WIDTH_KEY, String(w));
        return w;
      });
    };
    window.addEventListener('mousemove', handleMove);
    window.addEventListener('mouseup', handleUp);
  };

  useEffect(() => {
    invoke<SessionView>('get_session_view', { sessionId }).then(setSession);
    invoke<SessionBlock[]>('list_session_blocks', { sessionId }).then(setBlocks);
  }, [sessionId]);

  useEffect(() => {
    invoke<Account[]>('list_accounts').then(setAccounts).catch(() => setAccounts([]));
    invoke<SessionView[]>('list_workspace_sessions').then(setAllSessions).catch(() => setAllSessions([]));
    invoke<AppSettings>('get_app_settings').then(setAppSettings).catch(() => {});
  }, []);

  useEffect(() => {
    invoke<PullRequestInfo | null>('get_pull_request', { sessionId }).then(setPullRequest).catch(() => setPullRequest(null));
  }, [sessionId]);

  useEffect(() => {
    const unlisten = listen<{ sessionId: string; comments: PrComment[] }>('pr-comments-found', (event) => {
      if (event.payload.sessionId !== sessionId) return;
      setNewPrComments((prev) => [...prev, ...event.payload.comments]);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, [sessionId]);

  const handleOpenPrDialog = () => {
    setPrDialogOpen(true);
    setPrError(null);
    setPrPreview(null);
    setPrTitle('');
    if (!session) return;
    setLoadingPrPreview(true);
    invoke<PullRequestPreview>('preview_pull_request', { sessionId: session.id })
      .then((preview) => {
        setPrPreview(preview);
        setPrTitle(preview.suggestedTitle);
      })
      .catch((e) => setPrError(String(e)))
      .finally(() => setLoadingPrPreview(false));
  };

  const handleCreatePr = async () => {
    if (!session || creatingPr) return;
    setCreatingPr(true);
    setPrError(null);
    try {
      const pr = await invoke<PullRequestInfo>('create_pull_request', { sessionId: session.id, title: prTitle.trim() || undefined });
      setPullRequest(pr);
      setPrDialogOpen(false);
    } catch (e) {
      setPrError(String(e));
    } finally {
      setCreatingPr(false);
    }
  };

  const handleCheckPrComments = async () => {
    if (!session || checkingPrComments) return;
    setCheckingPrComments(true);
    setPrError(null);
    try {
      const comments = await invoke<PrComment[]>('check_pr_comments', { sessionId: session.id });
      if (comments.length > 0) setNewPrComments((prev) => [...prev, ...comments]);
    } catch (e) {
      setPrError(String(e));
    } finally {
      setCheckingPrComments(false);
    }
  };

  const handleSendPrCommentsToAgent = async () => {
    if (!session || newPrComments.length === 0 || sendingPrComments) return;
    setSendingPrComments(true);
    try {
      // The backend resolves the session actually checked out on this PR's
      // real branch — for a PR this session itself opened that's just this
      // session, but it may land somewhere else (a dedicated subsession),
      // so follow it there rather than assuming it stayed here.
      const targetSessionId = await invoke<string>('send_pr_comments_to_session', { commentIds: newPrComments.map((c) => c.id) });
      setNewPrComments([]);
      if (targetSessionId !== session.id) {
        navigate('agent-panel', { sessionId: targetSessionId });
      }
    } catch (e) {
      setPrError(String(e));
    } finally {
      setSendingPrComments(false);
    }
  };

  const handleDismissPrComments = async () => {
    if (newPrComments.length === 0) return;
    const ids = newPrComments.map((c) => c.id);
    setNewPrComments([]);
    try {
      await invoke('dismiss_pr_comments', { commentIds: ids });
    } catch (e) {
      setPrError(String(e));
    }
  };

  const handleReviewNow = async () => {
    if (!session || reviewing) return;
    setReviewing(true);
    setError(null);
    try {
      const reviewSession = await invoke<Session>('trigger_review', { sessionId: session.id });
      navigate('agent-panel', { sessionId: reviewSession.id });
    } catch (e) {
      setError(String(e));
    } finally {
      setReviewing(false);
    }
  };

  useEffect(() => {
    if (!filesOpen) return;
    invoke<FileTreeNode[]>('list_session_files', { sessionId }).then(setFileTree).catch(() => setFileTree([]));
    // Re-fetch whenever new activity arrives — a new block usually means a
    // tool ran, so the tree may have changed.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filesOpen, sessionId, blocks.length]);

  useEffect(() => {
    // Fetched unconditionally (not just when the sidecar is open) — the
    // timeline's own inline diff previews need this regardless of whether
    // the file explorer sidebar is toggled on.
    invoke<FileEdit[]>('list_file_edits', { sessionId }).then(setAllFileEdits).catch(() => setAllFileEdits([]));
  }, [sessionId, blocks.length]);

  useEffect(() => {
    invoke<TurnUsage[]>('list_turn_usage', { sessionId }).then(setAllTurnUsage).catch(() => setAllTurnUsage([]));
  }, [sessionId, blocks.length]);

  useEffect(() => {
    if (!session) return;
    const sevenDaysAgo = new Date(Date.now() - 7 * 24 * 60 * 60 * 1000).toISOString();
    invoke<AccountUsageSummary>('get_account_usage_summary', { accountId: session.accountId, sinceIso: sevenDaysAgo })
      .then(setAccountWeeklyUsage)
      .catch(() => setAccountWeeklyUsage(null));
  }, [session?.accountId, blocks.length]);

  const editsByBlockId = useMemo(() => {
    const map = new Map<string, FileEdit[]>();
    for (const edit of allFileEdits) {
      if (!edit.blockId) continue;
      const list = map.get(edit.blockId);
      if (list) list.push(edit);
      else map.set(edit.blockId, [edit]);
    }
    return map;
  }, [allFileEdits]);

  const usageByBlockId = useMemo(() => {
    const map = new Map<string, TurnUsage>();
    for (const usage of allTurnUsage) {
      if (usage.blockId) map.set(usage.blockId, usage);
    }
    return map;
  }, [allTurnUsage]);

  const sessionUsage = useMemo(() => {
    return allTurnUsage.reduce(
      (acc, u) => ({
        inputTokens: acc.inputTokens + u.inputTokens,
        outputTokens: acc.outputTokens + u.outputTokens,
        costUsd: acc.costUsd + (u.costUsd ?? 0),
      }),
      { inputTokens: 0, outputTokens: 0, costUsd: 0 },
    );
  }, [allTurnUsage]);

  const latestContextTokens = useMemo(() => {
    const last = allTurnUsage[allTurnUsage.length - 1];
    if (!last) return 0;
    return last.inputTokens + (last.cacheReadTokens ?? 0) + (last.cacheCreationTokens ?? 0);
  }, [allTurnUsage]);

  const toggleTurnCollapsed = (turnId: string) => {
    setCollapsedTurns((prev) => {
      const next = new Set(prev);
      if (next.has(turnId)) next.delete(turnId);
      else next.add(turnId);
      return next;
    });
  };

  useEffect(() => {
    if (!selectedFilePath) return;
    const filtered = allFileEdits.filter((e) => e.filePath === selectedFilePath || e.filePath.endsWith(`/${selectedFilePath}`));
    setFileEdits(filtered);
    // Only pick a default tab the moment a *different* file gets selected —
    // a later refetch (triggered by new blocks arriving) shouldn't fight a
    // tab the user already chose for the file they're looking at.
    if (lastSelectedFileRef.current !== selectedFilePath) {
      lastSelectedFileRef.current = selectedFilePath;
      setContentTab(filtered.length > 0 ? 'diff' : 'file');
    }
  }, [selectedFilePath, allFileEdits]);

  useEffect(() => {
    if (!selectedFilePath) {
      setFileContent(null);
      setFileContentError(null);
      return;
    }
    invoke<string>('read_session_file', { sessionId, path: selectedFilePath })
      .then((content) => {
        setFileContent(content);
        setFileContentError(null);
      })
      .catch((e) => {
        setFileContent(null);
        setFileContentError(String(e));
      });
  }, [selectedFilePath, sessionId]);

  const toggleCollapsed = (path: string) => {
    setCollapsedPaths((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });
  };

  useEffect(() => {
    if (!session) return;
    invoke<ModelOption[]>('list_provider_models', { accountId: session.accountId })
      .then(setModels)
      .catch(() => setModels([]));
    // Only re-fetch when the account changes, not on every session field update.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session?.accountId]);

  useEffect(() => {
    const unlistenOutput = listen<{ sessionId: string; kind: string; text: string }>('session-output', (event) => {
      if (event.payload.sessionId !== sessionId) return;
      setBlocks((prev) => [
        ...prev,
        {
          id: `${sessionId}-${prev.length}-${Date.now()}`,
          sessionId,
          kind: event.payload.kind as BlockKind,
          text: event.payload.text,
          createdAt: new Date().toISOString(),
        },
      ]);
    });
    const unlistenStatus = listen<[string, string]>('session-status', (event) => {
      const [sid, status] = event.payload;
      if (sid !== sessionId) return;
      setSession((prev) => (prev ? { ...prev, status: status as SessionStatus } : prev));
    });
    const unlistenPermission = listen<PermissionRequest>('permission-request', (event) => {
      if (event.payload.sessionId !== sessionId) return;
      setPendingApproval(event.payload);
    });
    return () => {
      unlistenOutput.then((f) => f());
      unlistenStatus.then((f) => f());
      unlistenPermission.then((f) => f());
    };
  }, [sessionId]);

  useEffect(() => {
    pendingApprovalRef.current = pendingApproval;
  }, [pendingApproval]);

  useEffect(() => {
    // Runs only on true unmount (empty deps) — an approval the user never
    // answered before navigating away or closing the session must resolve
    // as denied, not leave the hook (and the turn) hanging server-side.
    return () => {
      if (pendingApprovalRef.current) {
        invoke('respond_to_permission_request', { requestId: pendingApprovalRef.current.requestId, allow: false }).catch(() => {});
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    timelineRef.current?.scrollTo({ top: timelineRef.current.scrollHeight });
  }, [blocks, showThinking]);

  const visibleBlocks = useMemo(() => blocks.filter((b) => showThinking || b.kind !== 'think'), [blocks, showThinking]);
  const turns = useMemo(() => groupBlocksIntoTurns(visibleBlocks), [visibleBlocks]);
  const lastBlockId = visibleBlocks[visibleBlocks.length - 1]?.id;
  const isRunning = session?.status === 'running';

  // Claude Code's stream-json output is message-level, not token-level — it
  // prints nothing while a step (a long "thinking" pass, a big tool call) is
  // still being generated, so a silent gap of even several minutes at high
  // effort is normal, not proof of a hang. This elapsed-since-last-block
  // clock is the only signal a user has to tell "still normal" from
  // "actually stuck" during that gap, since there's no incremental progress
  // to show instead.
  const [workingClockNow, setWorkingClockNow] = useState(() => Date.now());
  useEffect(() => {
    if (!isRunning) return;
    const id = setInterval(() => setWorkingClockNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, [isRunning]);
  const lastBlockAt = blocks[blocks.length - 1]?.createdAt;
  const workingSeconds = isRunning && lastBlockAt ? Math.max(0, Math.floor((workingClockNow - new Date(lastBlockAt).getTime()) / 1000)) : 0;
  const account = accounts.find((a) => a.id === session?.accountId);
  const isSubscriptionAccount = account?.billing === 'sub';
  const realUsage = useRealUsage(isSubscriptionAccount ? account?.id : undefined, isSubscriptionAccount);

  const persistSettings = async (next: Partial<Pick<SessionView, 'model' | 'effort' | 'mode'>>) => {
    if (!session) return;
    const updated = { ...session, ...next };
    setSession(updated);
    await invoke('update_session_settings', {
      sessionId: session.id,
      model: updated.model,
      effort: updated.effort,
      mode: updated.mode,
    });
  };

  const attachmentsUploading = attachments.some((a) => !a.savedPath);

  const handleSend = async () => {
    if (!session || (!text.trim() && attachments.length === 0) || sending || isRunning || attachmentsUploading) return;
    const attachmentText = attachments
      .filter((a): a is Attachment & { savedPath: string } => a.savedPath !== null)
      .map((a) => `\n\n[Attached image: ${a.savedPath}]`)
      .join('');
    const value = text.trim() + attachmentText;
    setText('');
    attachments.forEach((a) => URL.revokeObjectURL(a.previewUrl));
    setAttachments([]);
    setSending(true);
    setError(null);
    try {
      await invoke('send_message', { sessionId: session.id, text: value });
    } catch (e) {
      setError(String(e));
    } finally {
      setSending(false);
    }
  };

  const addImageFile = async (file: File) => {
    if (!session) return;
    const id = crypto.randomUUID();
    const previewUrl = URL.createObjectURL(file);
    setAttachments((prev) => [...prev, { id, name: file.name || 'pasted-image.png', previewUrl, savedPath: null }]);
    try {
      const buffer = await file.arrayBuffer();
      const data = Array.from(new Uint8Array(buffer));
      const path = await invoke<string>('save_attachment', {
        sessionId: session.id,
        fileName: file.name || `${id}.png`,
        data,
      });
      setAttachments((prev) => prev.map((a) => (a.id === id ? { ...a, savedPath: path } : a)));
    } catch (e) {
      setError(String(e));
      setAttachments((prev) => prev.filter((a) => a.id !== id));
      URL.revokeObjectURL(previewUrl);
    }
  };

  const removeAttachment = (id: string) => {
    setAttachments((prev) => {
      const found = prev.find((a) => a.id === id);
      if (found) URL.revokeObjectURL(found.previewUrl);
      return prev.filter((a) => a.id !== id);
    });
  };

  const handlePaste = (e: React.ClipboardEvent<HTMLTextAreaElement>) => {
    const imageItems = Array.from(e.clipboardData.items).filter((item) => item.type.startsWith('image/'));
    if (imageItems.length === 0) return;
    e.preventDefault();
    imageItems.forEach((item) => {
      const file = item.getAsFile();
      if (file) addImageFile(file);
    });
  };

  const handleDrop = (e: React.DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    const imageFiles = Array.from(e.dataTransfer.files).filter((file) => file.type.startsWith('image/'));
    imageFiles.forEach((file) => addImageFile(file));
  };

  const openResumePicker = async () => {
    if (!session) return;
    setResumeOpen(true);
    setLoadingResume(true);
    try {
      const options = await invoke<ResumableConversation[]>('list_resumable_conversations', { sessionId: session.id });
      setResumeOptions(options);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingResume(false);
    }
  };

  const handleResumeSelect = async (providerSessionId: string) => {
    if (!session || resumingId) return;
    setResumingId(providerSessionId);
    try {
      await invoke('set_session_resume_target', { sessionId: session.id, providerSessionId });
      setResumeOpen(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setResumingId(null);
    }
  };

  const handleDelete = async () => {
    if (!session) return;
    try {
      await invoke('delete_session', { sessionId: session.id });
      onClose();
    } catch (e) {
      setError(String(e));
    }
  };

  const handleStop = async () => {
    if (!session) return;
    setStopping(true);
    try {
      await invoke('stop_session', { sessionId: session.id });
      setSession({ ...session, status: 'done' });
    } catch (e) {
      setError(String(e));
    } finally {
      setStopping(false);
    }
  };

  const respondToPermission = async (allow: boolean) => {
    if (!pendingApproval) return;
    setRespondingApproval(true);
    try {
      await invoke('respond_to_permission_request', { requestId: pendingApproval.requestId, allow });
      setPendingApproval(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setRespondingApproval(false);
    }
  };

  const blockTagInfo = (b: Pick<SessionBlock, 'kind'>, severity: Severity) => ({
    label:
      b.kind === 'you' ? 'YOU' : b.kind === 'think' ? 'THINKING' : b.kind === 'edit' ? 'EDIT' : b.kind === 'cmd' ? 'RUN' : b.kind === 'sum' ? 'CHILD SUMMARY' : b.kind === 'rev' ? 'REVIEW' : b.kind === 'agent' ? 'AGENT' : 'OUT',
    prefix: severity === 'error' ? '⚠ ' : b.kind === 'cmd' ? '● ' : b.kind === 'out' ? '⎿ ' : '',
  });

  const renderBlock = (b: SessionBlock) => {
    const severity = severityOfBlock(b);
    const tagMeta = severity === 'error' ? ERROR_TAG_STYLE : TAG_STYLE[b.kind];
    const isLast = b.id === lastBlockId;
    const { label: tagLabel, prefix: tagPrefix } = blockTagInfo(b, severity);

    const blockEdits = b.kind === 'edit' ? editsByBlockId.get(b.id) : undefined;
    if (blockEdits && blockEdits.length > 0) {
      const primary = blockEdits[0];
      const stats = blockEdits.reduce(
        (acc, e) => {
          const s = diffStats(e.oldText, e.newText);
          return { additions: acc.additions + s.additions, removals: acc.removals + s.removals };
        },
        { additions: 0, removals: 0 },
      );
      const icon = fileIcon(primary.filePath.split('/').pop() ?? primary.filePath);
      const preview = diffLines(primary.oldText, primary.newText, 1);
      const previewCap = 6;
      const hasMore = preview.length > previewCap || blockEdits.length > 1;
      return (
        <div key={b.id} className={styles.block} style={wrapStyle(b.kind, severity)}>
          <div className={styles.blockHead}>
            <span className={styles.bullet}>●</span>
            <span className={styles.fileIconBadge} style={{ color: icon.color }}>
              {icon.letter}
            </span>
            <span className={styles.editPath}>{primary.filePath}</span>
            {stats.additions > 0 && <span style={{ color: 'var(--grn)', font: '600 10px var(--font-mono)' }}>+{stats.additions}</span>}
            {stats.removals > 0 && <span style={{ color: 'var(--red)', font: '600 10px var(--font-mono)' }}>-{stats.removals}</span>}
            <span className={styles.time}>{formatTime(b.createdAt)}</span>
            {isLast && isRunning && <span className={styles.liveDot} />}
          </div>
          <div className={styles.diffConnector}>
            <DiffLinesView lines={preview.slice(0, previewCap)} />
            {hasMore && (
              <button
                type="button"
                className={styles.viewFullDiffLink}
                onClick={() => {
                  setFilesOpen(true);
                  setSelectedFilePath(primary.filePath);
                  setContentTab('diff');
                }}
              >
                view full diff →
              </button>
            )}
          </div>
        </div>
      );
    }

    return (
      <div key={b.id} className={styles.block} style={wrapStyle(b.kind, severity)}>
        <div className={styles.blockHead}>
          <span className={styles.tag} style={{ color: tagMeta.color, background: tagMeta.bg }}>
            {tagPrefix}
            {tagLabel}
          </span>
          <span className={styles.time}>{formatTime(b.createdAt)}</span>
          {isLast && isRunning && <span className={styles.liveDot} />}
        </div>
        <div className={styles.blockText} style={{ color: textColor(b.kind, severity) }}>
          {b.text}
        </div>
      </div>
    );
  };

  /** The turn's leading block (normally 'you', the prompt) doubles as the
   * accordion's header — clicking it collapses/expands everything else in
   * the turn, and it carries the whole turn's token usage (summed across
   * every block in it, since usage is recorded against the 'agent' block
   * but reads most naturally attached to "the prompt that caused it"). */
  const renderTurnHeader = (
    b: SessionBlock,
    opts: { collapsed: boolean; onToggle: () => void; turnUsage: { inputTokens: number; outputTokens: number } | null },
  ) => {
    const severity = severityOfBlock(b);
    const tagMeta = severity === 'error' ? ERROR_TAG_STYLE : TAG_STYLE[b.kind];
    const isLast = b.id === lastBlockId;
    const { label: tagLabel, prefix: tagPrefix } = blockTagInfo(b, severity);
    const { collapsed, onToggle, turnUsage } = opts;
    return (
      <div key={b.id} className={styles.block} style={wrapStyle(b.kind, severity)}>
        <button type="button" className={styles.blockHeadButton} onClick={onToggle}>
          <span className={styles.turnChevron}>{collapsed ? '▸' : '▾'}</span>
          <span className={styles.tag} style={{ color: tagMeta.color, background: tagMeta.bg }}>
            {tagPrefix}
            {tagLabel}
          </span>
          {turnUsage && (turnUsage.inputTokens > 0 || turnUsage.outputTokens > 0) && (
            <span className={styles.usageTag} title={`${turnUsage.inputTokens} in / ${turnUsage.outputTokens} out`}>
              ↑{formatTokenCount(turnUsage.inputTokens)} ↓{formatTokenCount(turnUsage.outputTokens)}
            </span>
          )}
          <span className={styles.time}>{formatTime(b.createdAt)}</span>
          {isLast && isRunning && <span className={styles.liveDot} />}
        </button>
        <div className={styles.blockText} style={{ color: textColor(b.kind, severity) }}>
          {b.text}
        </div>
      </div>
    );
  };

  if (!session) {
    return (
      <div className={styles.screen} style={embedded ? { height: '100%' } : undefined}>
        <div className={styles.header}>
          <Button variant="outline" size="icon" className={styles.backBtn} onClick={onClose}>
            {closeIcon}
          </Button>
          <span className={styles.title}>loading session…</span>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.screen} style={embedded ? { height: '100%' } : undefined}>
      <div className={styles.header}>
        <Button variant="outline" size="icon" className={styles.backBtn} title={closeTitle} onClick={onClose}>
          {closeIcon}
        </Button>
        <span className={styles.title}>{session.name || session.projectName}</span>
        <span className={styles.projectTag}>{session.projectName}</span>
        <span className={styles.projectTag}>{session.status}</span>
        {session.worktreeKind && (
          <span className={styles.projectTag} title={session.worktreeBranch ?? undefined}>
            {session.worktreeKind === 'linked' ? `⑃ ${session.worktreeBranch}` : '⑃ copy'}
          </span>
        )}
        <span className={styles.spacer} />
        <span className={styles.accountLabel} title="Account rotation lands in a later phase">
          {session.accountName}
        </span>
        <Select value={session.model} onValueChange={(value) => persistSettings({ model: value })}>
          <SelectTrigger size="sm" className={styles.headerSelect}>
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {models.length === 0 && session.model && <SelectItem value={session.model}>{session.model}</SelectItem>}
            {models.map((m) => (
              <SelectItem key={m.id} value={m.id}>
                {m.label}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Select value={session.effort} onValueChange={(value) => persistSettings({ effort: value as Effort })}>
          <SelectTrigger size="sm" className={styles.headerSelect}>
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
        <Select value={session.mode} onValueChange={(value) => persistSettings({ mode: value as SessionMode })}>
          <SelectTrigger size="sm" className={styles.headerSelect}>
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
        <Button
          variant="outline"
          className={`${styles.thinkingBtn} ${showThinking ? styles.on : ''}`}
          onClick={() => setShowThinking((v) => !v)}
        >
          ✦ thinking
        </Button>
        <Button
          variant="outline"
          className={styles.thinkingBtn}
          disabled={isRunning}
          onClick={openResumePicker}
          title="Continue a different Claude Code conversation from this project instead"
        >
          ↻ resume
        </Button>
        <Button
          variant="outline"
          className={`${styles.thinkingBtn} ${filesOpen ? styles.on : ''}`}
          onClick={() => setFilesOpen((v) => !v)}
          title="Toggle the file explorer sidebar"
        >
          ⌂ files
        </Button>
        <CreateSubsessionDialog
          trigger={
            <Button
              variant="outline"
              className={styles.thinkingBtn}
              title="Branch this session into a subsession working in its own worktree"
            >
              + subsession
            </Button>
          }
          parentSession={session}
          accounts={accounts}
          onCreated={(created) => navigate('agent-panel', { sessionId: created.id })}
        />
        <Button
          variant="outline"
          className={styles.thinkingBtn}
          disabled={!appSettings.reviewerEnabled || !appSettings.reviewerAccountId || reviewing}
          onClick={handleReviewNow}
          title={
            appSettings.reviewerEnabled && appSettings.reviewerAccountId
              ? 'Spawn a plan-mode review of this session with the configured reviewer account'
              : 'Configure a reviewer account in Settings → General first'
          }
        >
          {reviewing ? '…' : '◎ review now'}
        </Button>
        {pullRequest ? (
          <>
            <Button
              variant="outline"
              className={styles.thinkingBtn}
              onClick={() => openUrl(pullRequest.url)}
              title={`Open PR #${pullRequest.number} on GitHub`}
            >
              PR #{pullRequest.number} ↗
            </Button>
            <Button
              variant="outline"
              className={styles.thinkingBtn}
              disabled={checkingPrComments}
              onClick={handleCheckPrComments}
              title="Check this PR for new review comments now"
            >
              {checkingPrComments ? '…' : '↻ check comments'}
            </Button>
          </>
        ) : (
          <Button variant="outline" className={styles.thinkingBtn} onClick={handleOpenPrDialog} title="Push this branch and open a pull request">
            ⇡ create PR
          </Button>
        )}
        {isRunning && (
          <Button
            variant="destructive"
            size="sm"
            onClick={handleStop}
            disabled={stopping}
            title="Stop this turn — recovers a stuck session without deleting it"
          >
            {stopping ? '…' : '■ stop'}
          </Button>
        )}
        <ConfirmDeleteDialog
          trigger={
            <Button variant="ghost" size="icon" title="Remove session" style={{ color: 'var(--dim)' }}>
              🗑
            </Button>
          }
          title="Delete this session?"
          description={(() => {
            const descendantCount = countSessionSubtree(allSessions, session.id);
            return descendantCount > 0
              ? `This also removes ${descendantCount} subsession${descendantCount === 1 ? '' : 's'} and their worktree folders, plus this session's own transcript. This cannot be undone.`
              : "This removes the session and its full transcript. This cannot be undone.";
          })()}
          onConfirm={handleDelete}
        />
      </div>

      {(() => {
        // Prefer real numbers scraped from Claude Code's own `/usage` when
        // we have them (see src-tauri/src/usage_scraper.rs) — fall back to
        // Hangar's own token-based estimate for any field that's null
        // (couldn't be parsed) or while the scrape is still pending, so the
        // bars never go blank or block on a slow background fetch.
        const real = realUsage.stats;

        const estimatedContextPct = (latestContextTokens / contextWindowFor(session.provider)) * 100;
        const contextPct = real?.contextPct ?? estimatedContextPct;
        const contextIsReal = real?.contextPct != null;
        const contextColor = contextSeverityColor(contextSeverity(contextPct));

        const sessionTotal = sessionUsage.inputTokens + sessionUsage.outputTokens;
        const estimatedSessionPct = (sessionTotal / sessionTokenBudgetFor(session.provider)) * 100;
        const sessionPct = real?.sessionPct ?? estimatedSessionPct;
        const sessionIsReal = real?.sessionPct != null;

        const weeklyTotal = accountWeeklyUsage ? accountWeeklyUsage.inputTokens + accountWeeklyUsage.outputTokens : 0;
        const estimatedWeeklyPct = (weeklyTotal / weeklyTokenBudgetFor(session.provider)) * 100;
        const weeklyPct = real?.weeklyPct ?? estimatedWeeklyPct;
        const weeklyIsReal = real?.weeklyPct != null;

        return (
          <div className={styles.statusLine}>
            <UsageBar
              pct={contextPct}
              color={contextColor}
              label={`${Math.min(100, contextPct).toFixed(0)}% context`}
              title={contextIsReal ? '/usage — real context usage' : `~${formatTokenCount(latestContextTokens)} tokens of context used (estimate)`}
            />
            <span className={styles.statusLineDivider}>·</span>
            {isSubscriptionAccount ? (
              <UsageBar
                pct={sessionPct}
                color={contextSeverityColor(contextSeverity(sessionPct))}
                label={`${Math.min(100, sessionPct).toFixed(0)}% session`}
                title={
                  sessionIsReal
                    ? '/usage — real session usage'
                    : `↑${formatTokenCount(sessionUsage.inputTokens)} ↓${formatTokenCount(sessionUsage.outputTokens)} tokens this session — against an estimated budget, not a real provider limit`
                }
              />
            ) : (
              <span className={styles.statusLineText}>{formatUsd(sessionUsage.costUsd)} session</span>
            )}
            {(accountWeeklyUsage || weeklyIsReal) && (
              <>
                <span className={styles.statusLineDivider}>·</span>
                {isSubscriptionAccount ? (
                  <UsageBar
                    pct={weeklyPct}
                    color={contextSeverityColor(contextSeverity(weeklyPct))}
                    label={`${Math.min(100, weeklyPct).toFixed(0)}% / 7d`}
                    title={
                      weeklyIsReal
                        ? '/usage — real weekly usage'
                        : `↑${formatTokenCount(accountWeeklyUsage?.inputTokens ?? 0)} ↓${formatTokenCount(accountWeeklyUsage?.outputTokens ?? 0)} tokens this week — against an estimated budget, not a real provider quota`
                    }
                  />
                ) : (
                  <span className={styles.statusLineText}>{formatUsd(accountWeeklyUsage?.costUsd ?? 0)} / 7d</span>
                )}
              </>
            )}
            {isSubscriptionAccount && (
              <button
                type="button"
                className={styles.usageRefreshBtn}
                onClick={() => realUsage.refresh()}
                disabled={realUsage.loading}
                title="Re-check real /usage — spawns Claude Code interactively, takes a few seconds"
              >
                {realUsage.loading ? '…' : '↻'}
              </button>
            )}
          </div>
        );
      })()}

      <Dialog open={prDialogOpen} onOpenChange={setPrDialogOpen}>
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>Create pull request</DialogTitle>
            <DialogDescription>
              Pushes this session's branch (creating one first if it's still sitting on the repo's base
              branch) and opens a PR via <code>gh</code>. Any uncommitted changes are committed as one
              commit — there's no staging UI yet, everything goes in.
            </DialogDescription>
          </DialogHeader>

          {loadingPrPreview && <Spinner label="previewing changes…" />}

          {prPreview && !loadingPrPreview && (
            <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                <Label style={{ font: '600 9.5px var(--font-mono)', letterSpacing: '0.13em', color: 'var(--dim)' }}>TITLE</Label>
                <Input value={prTitle} onChange={(e) => setPrTitle(e.target.value)} />
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
                <Label style={{ font: '600 9.5px var(--font-mono)', letterSpacing: '0.13em', color: 'var(--dim)' }}>
                  BODY <span style={{ color: 'var(--acc)' }}>· grouped by change type</span>
                </Label>
                <div
                  style={{
                    font: '400 11px var(--font-mono)',
                    lineHeight: 1.7,
                    padding: '11px 13px',
                    borderRadius: 7,
                    border: '1px solid var(--line)',
                    background: 'var(--bg2)',
                    color: 'var(--mut)',
                    maxHeight: 220,
                    overflow: 'auto',
                  }}
                >
                  {prPreview.added.length === 0 && prPreview.modified.length === 0 && prPreview.removed.length === 0 && (
                    <span>No changes detected against the base branch.</span>
                  )}
                  {prPreview.added.length > 0 && (
                    <div>
                      <span style={{ color: 'var(--grn)' }}>### Added</span>
                      {prPreview.added.map((p) => (
                        <div key={p}>- {p}</div>
                      ))}
                    </div>
                  )}
                  {prPreview.modified.length > 0 && (
                    <div>
                      <span style={{ color: 'var(--amber)' }}>### Modified</span>
                      {prPreview.modified.map((p) => (
                        <div key={p}>- {p}</div>
                      ))}
                    </div>
                  )}
                  {prPreview.removed.length > 0 && (
                    <div>
                      <span style={{ color: 'var(--red)' }}>### Removed</span>
                      {prPreview.removed.map((p) => (
                        <div key={p}>- {p}</div>
                      ))}
                    </div>
                  )}
                </div>
              </div>
              <span style={{ font: '400 10.5px var(--font-mono)', color: 'var(--dim)' }}>
                {prPreview.added.length + prPreview.modified.length + prPreview.removed.length} file
                {prPreview.added.length + prPreview.modified.length + prPreview.removed.length === 1 ? '' : 's'} ·{' '}
                <span style={{ color: 'var(--grn)' }}>+{prPreview.linesAdded}</span>{' '}
                <span style={{ color: 'var(--red)' }}>−{prPreview.linesRemoved}</span>
              </span>
            </div>
          )}

          {prError && <span style={{ color: 'var(--red)', font: '500 11px var(--font-mono)' }}>{prError}</span>}
          <DialogFooter>
            <Button disabled={creatingPr || loadingPrPreview || !prTitle.trim()} onClick={handleCreatePr}>
              {creatingPr ? 'Creating…' : '⇡ Create PR'}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <Dialog open={resumeOpen} onOpenChange={setResumeOpen}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Resume a different conversation</DialogTitle>
            <DialogDescription>
              Pick a past Claude Code conversation from this project. Your next message here will continue
              that conversation instead of this session's own history.
            </DialogDescription>
          </DialogHeader>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 8, maxHeight: 320, overflow: 'auto' }}>
            {loadingResume && <Spinner label="loading…" />}
            {!loadingResume && resumeOptions.length === 0 && (
              <span style={{ color: 'var(--dim)', font: '400 12px var(--font-mono)' }}>
                no other conversations found for this project
              </span>
            )}
            {resumeOptions.map((c) => {
              const isResuming = resumingId === c.providerSessionId;
              return (
                <button
                  key={c.providerSessionId}
                  type="button"
                  disabled={resumingId !== null}
                  onClick={() => handleResumeSelect(c.providerSessionId)}
                  style={{
                    textAlign: 'left',
                    padding: '8px 10px',
                    borderRadius: 7,
                    border: '1px solid var(--line)',
                    background: 'var(--panel2)',
                    cursor: resumingId !== null ? 'default' : 'pointer',
                    opacity: resumingId !== null && !isResuming ? 0.5 : 1,
                    display: 'flex',
                    flexDirection: 'column',
                    gap: 3,
                  }}
                >
                  {isResuming ? (
                    <Spinner label="resuming…" />
                  ) : (
                    <>
                      <span style={{ font: '400 11.5px var(--font-mono)', color: 'var(--txt)' }}>{c.preview}</span>
                      <span style={{ font: '400 10px var(--font-mono)', color: 'var(--dim)' }}>
                        {formatTime(c.lastActiveAt)}
                      </span>
                    </>
                  )}
                </button>
              );
            })}
          </div>
        </DialogContent>
      </Dialog>

      <div className={styles.bodyRow}>
        {filesOpen && (
          <FileTreePanel
            nodes={fileTree}
            selectedPath={selectedFilePath}
            collapsedPaths={collapsedPaths}
            onToggleCollapsed={toggleCollapsed}
            onSelectFile={setSelectedFilePath}
          />
        )}
        <div className={styles.middleColumn}>
      <div className={styles.timeline} ref={timelineRef}>
        {visibleBlocks.length === 0 && <div className={styles.empty}>no messages yet — say something below</div>}
        {turns.map((turn) => {
          const collapsed = collapsedTurns.has(turn.id);
          const [header, ...rest] = turn.blocks;
          const turnUsage = rest.reduce<{ inputTokens: number; outputTokens: number } | null>((acc, b) => {
            const u = usageByBlockId.get(b.id);
            if (!u) return acc;
            return { inputTokens: (acc?.inputTokens ?? 0) + u.inputTokens, outputTokens: (acc?.outputTokens ?? 0) + u.outputTokens };
          }, null);
          return (
            <div key={turn.id} className={styles.turnGroup}>
              {header && renderTurnHeader(header, { collapsed, onToggle: () => toggleTurnCollapsed(turn.id), turnUsage })}
              {!collapsed && rest.map((b) => renderBlock(b))}
            </div>
          );
        })}
      </div>

      {pendingApproval && (
        <div className={styles.permissionBanner}>
          <span className={styles.permissionText}>
            El agente quiere ejecutar <strong>{pendingApproval.toolName}</strong>
            {summarizePermissionInput(pendingApproval.toolName, pendingApproval.toolInput) && (
              <>: <code className={styles.permissionCode}>{summarizePermissionInput(pendingApproval.toolName, pendingApproval.toolInput)}</code></>
            )}
          </span>
          <Button size="sm" disabled={respondingApproval} onClick={() => respondToPermission(true)}>
            Aprobar
          </Button>
          <Button size="sm" variant="destructive" disabled={respondingApproval} onClick={() => respondToPermission(false)}>
            Rechazar
          </Button>
        </div>
      )}

      {newPrComments.length > 0 && (
        <div className={styles.permissionBanner}>
          <span className={styles.permissionText}>
            {newPrComments.length} new PR comment{newPrComments.length === 1 ? '' : 's'} —{' '}
            {newPrComments
              .slice(0, 2)
              .map((c) => c.author)
              .join(', ')}
            {newPrComments.length > 2 ? ', …' : ''}
          </span>
          <Button size="sm" disabled={sendingPrComments} onClick={handleSendPrCommentsToAgent}>
            {sendingPrComments ? 'Sending…' : 'Send to agent'}
          </Button>
          <Button size="sm" variant="outline" onClick={handleDismissPrComments}>
            Dismiss
          </Button>
        </div>
      )}

      <div className={styles.composer}>
        {isRunning && (
          <div className={styles.workingHint} title="Claude Code streams whole steps, not partial progress — a silent gap of several minutes on a high-effort/thinking turn is normal">
            <span className={styles.workingDot} />
            agent is working — {formatWorkingDuration(workingSeconds)} since last update — composer unlocks when it's idle
          </div>
        )}
        {error && <div className={styles.workingHint} style={{ color: 'var(--red)' }}>{error}</div>}
        {attachments.length > 0 && (
          <div style={{ display: 'flex', gap: 8, marginBottom: 8, flexWrap: 'wrap' }}>
            {attachments.map((a) => (
              <div
                key={a.id}
                style={{
                  position: 'relative',
                  display: 'flex',
                  alignItems: 'center',
                  gap: 6,
                  padding: '4px 8px 4px 4px',
                  borderRadius: 7,
                  border: '1px solid var(--line2)',
                  background: 'var(--panel2)',
                  opacity: a.savedPath ? 1 : 0.6,
                }}
              >
                <img src={a.previewUrl} alt={a.name} style={{ width: 28, height: 28, objectFit: 'cover', borderRadius: 4 }} />
                <span style={{ font: '400 10.5px var(--font-mono)', color: 'var(--mut)', maxWidth: 120, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                  {a.savedPath ? a.name : 'uploading…'}
                </span>
                <button
                  type="button"
                  onClick={() => removeAttachment(a.id)}
                  title="Remove attachment"
                  style={{ background: 'none', border: 'none', cursor: 'pointer', color: 'var(--dim)', font: '500 12px var(--font-mono)' }}
                >
                  ✕
                </button>
              </div>
            ))}
          </div>
        )}
        <div className={styles.composerBox} onDragOver={(e) => e.preventDefault()} onDrop={handleDrop}>
          <Textarea
            className={styles.composerInput}
            rows={1}
            value={text}
            disabled={isRunning || sending}
            placeholder="Ask to make changes, @mention files, run /commands… (paste or drop an image)"
            onChange={(e) => setText(e.target.value)}
            onPaste={handlePaste}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                handleSend();
              }
            }}
          />
          <span className={styles.kbdHint}>⌘L</span>
          <Button
            size="icon"
            className={styles.sendBtn}
            disabled={isRunning || sending || attachmentsUploading || (!text.trim() && attachments.length === 0)}
            onClick={handleSend}
          >
            ↑
          </Button>
        </div>
      </div>
        </div>
        {filesOpen && (
          <ContentPanel
            filePath={selectedFilePath}
            edits={fileEdits}
            content={fileContent}
            contentError={fileContentError}
            activeTab={contentTab}
            onTabChange={setContentTab}
            width={filesViewWidth}
            onResizeStart={startFilesViewResize}
          />
        )}
      </div>
    </div>
  );
}

function FileTreePanel({
  nodes,
  selectedPath,
  collapsedPaths,
  onToggleCollapsed,
  onSelectFile,
}: {
  nodes: FileTreeNode[];
  selectedPath: string | null;
  collapsedPaths: Set<string>;
  onToggleCollapsed: (path: string) => void;
  onSelectFile: (path: string) => void;
}) {
  return (
    <div className={`${styles.sidePanel} ${styles.filesSidePanel}`}>
      <div style={{ padding: '10px 10px 4px', font: '600 9.5px var(--font-mono)', letterSpacing: '0.12em', color: 'var(--dim)' }}>
        EXPLORER
      </div>
      {nodes.length === 0 ? (
        <div style={{ padding: 10, font: '400 11px var(--font-mono)', color: 'var(--dim)' }}>no files</div>
      ) : (
        <FileTreeRows
          nodes={nodes}
          selectedPath={selectedPath}
          collapsedPaths={collapsedPaths}
          onToggleCollapsed={onToggleCollapsed}
          onSelectFile={onSelectFile}
        />
      )}
    </div>
  );
}

export function DiffLinesView({ lines }: { lines: DiffLine[] }) {
  return (
    <div className={styles.diffLines}>
      {lines.map((line, i) => {
        if (line.type === 'collapsed') {
          return (
            <div key={`collapsed-${i}`} className={styles.diffCollapsed}>
              ⋯ {line.count} unchanged line{line.count === 1 ? '' : 's'} ⋯
            </div>
          );
        }
        const lineNo = line.type === 'remove' ? line.oldLine : line.newLine;
        const prefix = line.type === 'add' ? '+' : line.type === 'remove' ? '-' : ' ';
        return (
          <div
            key={i}
            className={`${styles.diffLine} ${line.type === 'add' ? styles.diffAdd : line.type === 'remove' ? styles.diffRemove : ''}`}
          >
            <span className={styles.diffLineNo}>{lineNo}</span>
            <span className={styles.diffPrefix}>{prefix}</span>
            <span className={styles.diffText}>{line.text}</span>
          </div>
        );
      })}
    </div>
  );
}

/** Read-only syntax-highlighted file view — line-numbered the same way
 * `DiffLinesView` numbers diff lines, for visual consistency between the
 * Diff and File tabs. Falls back to a plain (unhighlighted) grammar for
 * extensions Prism's bundled build doesn't ship, rather than erroring. */
function HighlightedFile({ code, prismLanguage }: { code: string; prismLanguage: string }) {
  const { resolvedTheme } = useTheme();
  return (
    <Highlight theme={resolvedTheme === 'light' ? themes.vsLight : themes.vsDark} code={code} language={prismLanguage}>
      {({ tokens, getTokenProps }) => (
        <div className={styles.fileContentPre}>
          {tokens.map((line, i) => (
            <div key={i} className={styles.diffLine}>
              <span className={styles.diffLineNo}>{i + 1}</span>
              <span className={styles.diffPrefix} />
              <span className={styles.diffText}>
                {line.map((token, key) => (
                  <span key={key} {...getTokenProps({ token })} />
                ))}
              </span>
            </div>
          ))}
        </div>
      )}
    </Highlight>
  );
}

function ContentPanel({
  filePath,
  edits,
  content,
  contentError,
  activeTab,
  onTabChange,
  width,
  onResizeStart,
}: {
  filePath: string | null;
  edits: FileEdit[];
  content: string | null;
  contentError: string | null;
  activeTab: 'diff' | 'file';
  onTabChange: (tab: 'diff' | 'file') => void;
  width: number;
  onResizeStart: (e: React.MouseEvent) => void;
}) {
  const language = filePath ? languageFor(filePath.split('/').pop() ?? filePath) : null;
  return (
    <div className={styles.sidePanel} style={{ width, padding: 10, display: 'flex', flexDirection: 'column', position: 'relative' }}>
      <div
        className={styles.resizeHandle}
        onMouseDown={onResizeStart}
        title="Drag to resize"
      />
      <div style={{ font: '600 9.5px var(--font-mono)', letterSpacing: '0.12em', color: 'var(--dim)', marginBottom: 8 }}>
        FILE VIEW
      </div>
      {!filePath && <div style={{ font: '400 11px var(--font-mono)', color: 'var(--dim)' }}>select a file to view it</div>}
      {filePath && (
        <>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 8 }}>
            <span style={{ font: '600 11px var(--font-mono)', color: 'var(--txt)', wordBreak: 'break-all', flex: 1 }}>
              {filePath}
            </span>
            {language && <span className={styles.languageTag}>{language.name}</span>}
          </div>
          {edits.length > 0 && (
            <div className={styles.contentTabs}>
              <button
                type="button"
                className={`${styles.contentTab} ${activeTab === 'diff' ? styles.contentTabActive : ''}`}
                onClick={() => onTabChange('diff')}
              >
                Diff
              </button>
              <button
                type="button"
                className={`${styles.contentTab} ${activeTab === 'file' ? styles.contentTabActive : ''}`}
                onClick={() => onTabChange('file')}
              >
                File
              </button>
            </div>
          )}
          {(edits.length === 0 || activeTab === 'file') &&
            (contentError ? (
              <div style={{ font: '400 11px var(--font-mono)', color: 'var(--dim)' }}>{contentError}</div>
            ) : content === null ? (
              <div style={{ font: '400 11px var(--font-mono)', color: 'var(--dim)' }}>loading…</div>
            ) : (
              <HighlightedFile code={content} prismLanguage={language?.prism ?? 'text'} />
            ))}
          {edits.length > 0 && activeTab === 'diff' && (
            <div style={{ overflow: 'auto' }}>
              {edits.map((edit) => (
                <div key={edit.id} style={{ marginBottom: 12, border: '1px solid var(--line)', borderRadius: 6, overflow: 'hidden' }}>
                  <div style={{ padding: '4px 8px', background: 'var(--panel2)', font: '500 9.5px var(--font-mono)', color: 'var(--dim)' }}>
                    {edit.toolName} · {formatTime(edit.createdAt)}
                  </div>
                  <DiffLinesView lines={diffLines(edit.oldText, edit.newText)} />
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}
