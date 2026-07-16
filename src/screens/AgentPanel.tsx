import { useEffect, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useNavigation } from '../nav/NavigationProvider';
import {
  EFFORT_OPTIONS,
  MODE_LABEL,
  type BlockKind,
  type Effort,
  type ModelOption,
  type ResumableConversation,
  type SessionBlock,
  type SessionMode,
  type SessionStatus,
  type SessionView,
} from '../types/session';
import { Button } from '../components/ui/button';
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '../components/ui/dialog';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Textarea } from '../components/ui/textarea';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import styles from './AgentPanel.module.css';

interface Attachment {
  id: string;
  name: string;
  previewUrl: string;
  savedPath: string | null;
}

const MODE_OPTIONS: SessionMode[] = ['plan', 'ask_edits', 'auto_edit', 'yolo'];

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

function wrapStyle(kind: BlockKind): React.CSSProperties {
  const base: React.CSSProperties = { opacity: kind === 'think' ? 0.8 : 1, fontStyle: kind === 'think' ? 'italic' : 'normal' };
  if (kind === 'you') {
    return { ...base, background: 'var(--panel)', border: '1px solid var(--line)' };
  }
  const borderLeft = kind === 'agent' || kind === 'sum' ? '2px solid var(--teal)' : kind === 'edit' ? '2px solid var(--grn)' : '2px solid var(--line)';
  return { ...base, background: 'transparent', border: '1px solid transparent', borderLeft };
}

function textColor(kind: BlockKind): string {
  if (kind === 'think') return 'var(--dim)';
  if (kind === 'you') return 'var(--txt)';
  return 'var(--mut)';
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
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
  const [session, setSession] = useState<SessionView | null>(null);
  const [blocks, setBlocks] = useState<SessionBlock[]>([]);
  const [models, setModels] = useState<ModelOption[]>([]);
  const [showThinking, setShowThinking] = useState(true);
  const [text, setText] = useState('');
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);
  const [attachments, setAttachments] = useState<Attachment[]>([]);
  const [resumeOpen, setResumeOpen] = useState(false);
  const [resumeOptions, setResumeOptions] = useState<ResumableConversation[]>([]);
  const [loadingResume, setLoadingResume] = useState(false);
  const timelineRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    invoke<SessionView>('get_session_view', { sessionId }).then(setSession);
    invoke<SessionBlock[]>('list_session_blocks', { sessionId }).then(setBlocks);
  }, [sessionId]);

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
    return () => {
      unlistenOutput.then((f) => f());
      unlistenStatus.then((f) => f());
    };
  }, [sessionId]);

  useEffect(() => {
    timelineRef.current?.scrollTo({ top: timelineRef.current.scrollHeight });
  }, [blocks, showThinking]);

  const visibleBlocks = useMemo(() => blocks.filter((b) => showThinking || b.kind !== 'think'), [blocks, showThinking]);
  const isRunning = session?.status === 'running';

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
    if (!session) return;
    try {
      await invoke('set_session_resume_target', { sessionId: session.id, providerSessionId });
      setResumeOpen(false);
    } catch (e) {
      setError(String(e));
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
          description="This removes the session and its full transcript. This cannot be undone."
          onConfirm={handleDelete}
        />
      </div>

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
            {loadingResume && <span style={{ color: 'var(--dim)', font: '400 12px var(--font-mono)' }}>loading…</span>}
            {!loadingResume && resumeOptions.length === 0 && (
              <span style={{ color: 'var(--dim)', font: '400 12px var(--font-mono)' }}>
                no other conversations found for this project
              </span>
            )}
            {resumeOptions.map((c) => (
              <button
                key={c.providerSessionId}
                type="button"
                onClick={() => handleResumeSelect(c.providerSessionId)}
                style={{
                  textAlign: 'left',
                  padding: '8px 10px',
                  borderRadius: 7,
                  border: '1px solid var(--line)',
                  background: 'var(--panel2)',
                  cursor: 'pointer',
                  display: 'flex',
                  flexDirection: 'column',
                  gap: 3,
                }}
              >
                <span style={{ font: '400 11.5px var(--font-mono)', color: 'var(--txt)' }}>{c.preview}</span>
                <span style={{ font: '400 10px var(--font-mono)', color: 'var(--dim)' }}>
                  {formatTime(c.lastActiveAt)}
                </span>
              </button>
            ))}
          </div>
        </DialogContent>
      </Dialog>

      <div className={styles.timeline} ref={timelineRef}>
        {visibleBlocks.length === 0 && <div className={styles.empty}>no messages yet — say something below</div>}
        {visibleBlocks.map((b, i) => {
          const tagMeta = TAG_STYLE[b.kind];
          const isLast = i === visibleBlocks.length - 1;
          return (
            <div key={b.id} className={styles.block} style={wrapStyle(b.kind)}>
              <div className={styles.blockHead}>
                <span className={styles.tag} style={{ color: tagMeta.color, background: tagMeta.bg }}>
                  {b.kind === 'you' ? 'YOU' : b.kind === 'think' ? 'THINKING' : b.kind === 'edit' ? 'EDIT' : b.kind === 'cmd' ? 'RUN' : b.kind === 'sum' ? 'CHILD SUMMARY' : b.kind === 'rev' ? 'REVIEW' : b.kind === 'agent' ? 'AGENT' : 'OUT'}
                </span>
                <span className={styles.time}>{formatTime(b.createdAt)}</span>
                {isLast && isRunning && <span className={styles.liveDot} />}
              </div>
              <div className={styles.blockText} style={{ color: textColor(b.kind) }}>
                {b.text}
              </div>
            </div>
          );
        })}
      </div>

      <div className={styles.composer}>
        {isRunning && (
          <div className={styles.workingHint}>
            <span className={styles.workingDot} />
            agent is working — composer unlocks when it's idle
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
  );
}
