import { useEffect, useRef, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import { invoke } from '../lib/invoke';
import { Button } from '../components/ui/button';
import { toastError, toastSuccess } from '../components/ErrorOverlay';
import { useNavigation } from '../nav/NavigationProvider';
import { PROVIDER_META, type Account } from '../types/account';
import type { AgentNode } from '../types/agentNode';
import { BLOCK_TAG, type Project, type Session, type SessionBlock } from '../types/session';
import styles from './ExecRun.module.css';

interface OutputEventPayload {
  sessionId: string;
  kind: string;
  text: string;
}

// Past this many sessions running at once, the header banner switches to
// an amber "hard to supervise" warning instead of a neutral count.
const WARN_THRESHOLD = 4;

const STATUS_COLOR: Record<Session['status'], string> = {
  running: 'var(--teal)',
  waiting: 'var(--dim)',
  reviewing: 'var(--acc)',
  done: 'var(--grn)',
};

const STATUS_BG: Record<Session['status'], string> = {
  running: 'var(--tealD)',
  waiting: 'var(--panel2)',
  reviewing: 'var(--accD)',
  done: 'var(--grnD)',
};

const BLOCK_TAG_STYLE: Record<SessionBlock['kind'], { color: string; bg: string }> = {
  you: { color: 'var(--acc)', bg: 'var(--accD)' },
  think: { color: 'var(--dim)', bg: 'var(--panel2)' },
  edit: { color: 'var(--grn)', bg: 'var(--grnD)' },
  cmd: { color: 'var(--mut)', bg: 'var(--panel2)' },
  agent: { color: 'var(--teal)', bg: 'var(--tealD)' },
  out: { color: 'var(--mut)', bg: 'var(--panel2)' },
  sum: { color: 'var(--teal)', bg: 'var(--tealD)' },
  rev: { color: 'var(--acc)', bg: 'var(--accD)' },
};

function blockBorderColor(kind: SessionBlock['kind']): string {
  if (kind === 'sum' || kind === 'agent') return 'var(--teal)';
  if (kind === 'rev' || kind === 'you') return 'var(--acc)';
  if (kind === 'edit') return 'var(--grn)';
  return 'var(--line2)';
}

function formatTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '';
  return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function BlockRow({ block }: { block: SessionBlock }) {
  const showTag = block.kind !== 'out';
  const boxed = block.kind === 'sum' || block.kind === 'rev';
  const tagMeta = BLOCK_TAG_STYLE[block.kind];
  return (
    <div className={`${styles.blockRow} ${boxed ? styles.blockRowTagged : ''}`} style={{ borderLeftColor: blockBorderColor(block.kind) }}>
      {showTag && (
        <div className={styles.blockHead}>
          <span className={styles.blockTag} style={{ color: tagMeta.color, background: tagMeta.bg }}>
            {BLOCK_TAG[block.kind]}
          </span>
          <span className={styles.blockTime}>{formatTime(block.createdAt)}</span>
        </div>
      )}
      <div className={styles.blockText} style={{ color: block.kind === 'cmd' ? 'var(--txt)' : 'var(--mut)' }}>
        {block.text}
      </div>
    </div>
  );
}

export function ExecRun() {
  const { current, navigate } = useNavigation();
  const projectId = current.params?.projectId as string | undefined;
  const rootId = current.params?.rootId as string | undefined;

  const [projectName, setProjectName] = useState('');
  const [nodes, setNodes] = useState<AgentNode[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [blocksBySession, setBlocksBySession] = useState<Record<string, SessionBlock[]>>({});
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());
  const [stopping, setStopping] = useState(false);
  const loadedBlocksRef = useRef<Set<string>>(new Set());
  const knownSessionIdsRef = useRef<Set<string>>(new Set());
  const rootEverRunningRef = useRef(false);

  useEffect(() => {
    invoke<Project[]>('list_projects')
      .then((list) => setProjectName(list.find((p) => p.id === projectId)?.name ?? ''))
      .catch(() => {});
    invoke<Account[]>('list_accounts').then(setAccounts).catch(() => {});
  }, [projectId]);

  const refresh = async () => {
    if (!projectId) return;
    try {
      const [nodeList, sessionList] = await Promise.all([
        invoke<AgentNode[]>('list_agent_nodes', { projectId }),
        invoke<Session[]>('list_project_sessions', { projectId }),
      ]);
      setNodes(nodeList);
      const nodeIds = new Set(nodeList.map((n) => n.id));
      const treeSessions = sessionList.filter((s) => s.nodeId && nodeIds.has(s.nodeId));
      setSessions(treeSessions);
      knownSessionIdsRef.current = new Set(treeSessions.map((s) => s.id));
      // Backfill the block backlog for any session seen for the first time
      // here — either the very first load, or one `ensure_session_for_node`
      // just created for a node the recursion reached since our last poll.
      for (const s of treeSessions) {
        if (loadedBlocksRef.current.has(s.id)) continue;
        loadedBlocksRef.current.add(s.id);
        invoke<SessionBlock[]>('list_session_blocks', { sessionId: s.id })
          .then((blocks) => setBlocksBySession((prev) => ({ ...prev, [s.id]: blocks.slice(-30) })))
          .catch(() => {});
      }
    } catch (e) {
      toastError(String(e));
    }
  };

  useEffect(() => {
    refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);

  // Live events below update already-known sessions instantly; this poll is
  // what discovers a session that didn't exist yet — a tree node's session
  // is only created lazily the first time the recursion actually reaches
  // it (`ensure_session_for_node` in agent_nodes.rs), so the initial load
  // can easily predate most of them.
  useEffect(() => {
    if (!projectId) return;
    const id = setInterval(refresh, 3000);
    return () => clearInterval(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [projectId]);

  useEffect(() => {
    const unlistenOutput = listen<OutputEventPayload>('session-output', (event) => {
      const { sessionId, kind, text } = event.payload;
      if (!knownSessionIdsRef.current.has(sessionId)) return;
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
      setSessions((prev) => prev.map((s) => (s.id === sessionId ? { ...s, status: status as Session['status'] } : s)));
    });
    return () => {
      unlistenOutput.then((f) => f());
      unlistenStatus.then((f) => f());
    };
  }, []);

  const rootSession = sessions.find((s) => s.nodeId === rootId);
  if (rootSession?.status === 'running') rootEverRunningRef.current = true;
  // A node's session is reused across runs (`ensure_session_for_node` picks
  // up the most recent one rather than making a fresh one each time), so on
  // a second run of the same tree the root's session can already be
  // "done" from last time before this run has even reached it — gating on
  // having actually observed it go `running` this mount avoids a false
  // "tree finished" the instant the screen opens.
  const rootFinished = rootEverRunningRef.current && rootSession != null && rootSession.status !== 'running';

  useEffect(() => {
    if (!rootFinished) return;
    const root = nodes.find((n) => n.id === rootId);
    toastSuccess(`Tree finished — "${root?.role ?? 'root'}" is done.`);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rootFinished]);

  const runningCount = sessions.filter((s) => s.status === 'running').length;

  const handleStopTree = async () => {
    const runningSessions = sessions.filter((s) => s.status === 'running');
    if (runningSessions.length === 0 || stopping) return;
    setStopping(true);
    try {
      await Promise.all(runningSessions.map((s) => invoke('stop_session', { sessionId: s.id })));
      toastSuccess('Stopped.');
    } catch (e) {
      toastError(String(e));
    } finally {
      setStopping(false);
    }
  };

  const toggleCollapsed = (nodeId: string, value: boolean) => {
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (value) next.add(nodeId);
      else next.delete(nodeId);
      return next;
    });
  };

  const orderedNodes = [...nodes].sort((a, b) => a.depth - b.depth || a.createdAt.localeCompare(b.createdAt));
  const visibleNodes = orderedNodes.filter((n) => !collapsed.has(n.id));
  const hiddenNodes = orderedNodes.filter((n) => collapsed.has(n.id));

  if (!projectId || !rootId) {
    return (
      <div className={styles.screen}>
        <div className={styles.emptyState}>
          <span>Missing project or tree to run — go back and press "▶ Run tree" again.</span>
        </div>
      </div>
    );
  }

  return (
    <div className={styles.screen}>
      <div className={styles.header}>
        <Button variant="outline" size="sm" className={styles.editBtn} onClick={() => navigate('main-view')} title="Back to the main view">
          ⌂ main
        </Button>
        <span className={styles.badge}>⌥</span>
        <span className={styles.breadcrumb}>
          <span className={styles.breadcrumbDim}>running /</span> {projectName || projectId}
        </span>
        <div className={styles.divider} />
        <span className={`${styles.warnBanner} ${runningCount >= WARN_THRESHOLD ? styles.warnBannerHigh : ''}`}>
          {runningCount > 0 && <span className={styles.warnDot} />}
          {runningCount === 0
            ? rootFinished
              ? 'tree finished'
              : 'no agents running yet'
            : `${runningCount} agent${runningCount === 1 ? '' : 's'} running${runningCount >= WARN_THRESHOLD ? ' — too many to supervise well' : ''}`}
        </span>
        <span className={styles.spacer} />
        <Button variant="outline" size="sm" className={styles.editBtn} onClick={() => navigate('tree-canvas', { projectId })}>
          ✎ edit tree
        </Button>
        <button className={styles.stopBtn} disabled={runningCount === 0 || stopping} onClick={handleStopTree}>
          {stopping ? '…' : '■ Stop tree'}
        </button>
      </div>

      <div className={styles.body}>
        <div className={styles.rail}>
          <span className={styles.railLabel}>MINIMIZED — STILL RUNNING</span>
          {hiddenNodes.map((n) => {
            const session = sessions.find((s) => s.nodeId === n.id);
            const isLive = session?.status === 'running';
            return (
              <button key={n.id} className={styles.pill} title="Restore panel" onClick={() => toggleCollapsed(n.id, false)}>
                <span className={`${styles.pillDot} ${isLive ? styles.pillDotLive : session?.status === 'done' ? styles.pillDotDone : ''}`} />
                <span className={styles.pillName}>{n.role}</span>
                <span className={styles.pillRestore}>⤢</span>
              </button>
            );
          })}
          <span className={styles.railHint}>Collapse any panel with ▁ — the agent keeps running in background.</span>
        </div>

        <div className={styles.grid}>
          {visibleNodes.map((n) => {
            const session = sessions.find((s) => s.nodeId === n.id);
            const account = session ? accounts.find((a) => a.id === session.accountId) : undefined;
            const prov = account ? PROVIDER_META[account.provider] : null;
            const parent = n.parentId ? nodes.find((p) => p.id === n.parentId) : null;
            const blocks = session ? (blocksBySession[session.id] ?? []) : [];
            const lastBlock = blocks[blocks.length - 1];
            const isRunning = session?.status === 'running';

            return (
              <div key={n.id} className={`${styles.card} ${session?.status === 'reviewing' ? styles.cardReviewing : ''}`}>
                <div className={styles.cardHeader}>
                  {prov && (
                    <span className={styles.provBadge} style={{ background: prov.tintVar, color: prov.colorVar }}>
                      {prov.letter}
                    </span>
                  )}
                  <span className={styles.roleName}>{n.role}</span>
                  <span className={styles.reportsTo}>↳ {parent?.role ?? 'you'}</span>
                  <span className={styles.spacer} />
                  {session && (
                    <>
                      <span className={styles.badgeSmall} style={{ background: 'var(--accD)', color: 'var(--acc)' }}>
                        {session.model}
                      </span>
                      <span className={styles.badgeSmall} style={{ background: 'var(--tealD)', color: 'var(--teal)' }}>
                        {account?.name ?? session.accountId}
                      </span>
                      <span className={styles.statusPill} style={{ background: STATUS_BG[session.status], color: STATUS_COLOR[session.status] }}>
                        {session.status}
                      </span>
                    </>
                  )}
                  {!session && (
                    <span className={styles.statusPill} style={{ background: 'var(--panel2)', color: 'var(--dim)' }}>
                      pending
                    </span>
                  )}
                  <button className={styles.collapseBtn} title="Collapse to rail" onClick={() => toggleCollapsed(n.id, true)}>
                    ▁
                  </button>
                </div>
                <div className={styles.cardBody}>
                  {!session && <span className={styles.notStarted}>not started yet — waiting on its children</span>}
                  {session && blocks.length === 0 && <span className={styles.notStarted}>no output yet</span>}
                  {blocks.map((b) => (
                    <BlockRow key={b.id} block={b} />
                  ))}
                </div>
                <div className={styles.cardFooter}>
                  <span className={`${styles.activityDot} ${isRunning ? styles.activityDotLive : ''}`} style={{ background: session ? STATUS_COLOR[session.status] : 'var(--line2)' }} />
                  <span className={styles.activity}>
                    {!session ? 'not started' : lastBlock ? lastBlock.text.slice(0, 80) : 'starting…'}
                  </span>
                  {session && <span className={styles.modelMeta}>{session.model}</span>}
                </div>
              </div>
            );
          })}

          {visibleNodes.length === 0 && <div className={styles.emptyState}>Nothing to show — every panel is minimized.</div>}
        </div>
      </div>
    </div>
  );
}
