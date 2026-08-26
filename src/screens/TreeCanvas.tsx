import { useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { Button } from '../components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Input } from '../components/ui/input';
import { Textarea } from '../components/ui/textarea';
import { Label } from '../components/ui/label';
import { Spinner } from '../components/Spinner';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import { useAgentTree, MAX_DEPTH, TEMPLATES } from '../hooks/useAgentTree';
import { useElementWidth } from '../hooks/useElementWidth';
import { useNodeEditor } from '../hooks/useNodeEditor';
import { useNavigation } from '../nav/NavigationProvider';
import { providerMeta, UNKNOWN_PROVIDER_META, type Account } from '../types/account';
import type { AgentNode, UpdateAgentNode } from '../types/agentNode';
import type { Session } from '../types/session';
import type { ProjectAccountRotation } from '../types/settings';
import styles from './TreeCanvas.module.css';

const CARD_W = 218;
const CARD_H = 92;
const SLOT = 250;
const TOP = 40;
const VGAP = 160;

type NodePos = { x: number; y: number };

/** Leaf-centering tree layout — a parent sits above the midpoint of its
 * children's x, leaves are laid out left-to-right in fixed-width slots.
 * Reimplements the same algorithm as the `TreeCanvas.dc.html` design
 * mockup, just keyed off the backend's own 0-indexed `depth` field instead
 * of the mockup's 1-indexed local recomputation. */
function computeLayout(nodes: AgentNode[], canvasWidth: number, editorOpen: boolean): Record<string, NodePos> {
  const childrenOf = (id: string | null) => nodes.filter((n) => n.parentId === id);
  const pos: Record<string, NodePos> = {};
  let cursor = 0;
  const place = (node: AgentNode): number => {
    const kids = childrenOf(node.id);
    let x: number;
    if (kids.length === 0) {
      x = cursor * SLOT + SLOT / 2;
      cursor++;
    } else {
      const xs = kids.map(place);
      x = (xs[0] + xs[xs.length - 1]) / 2;
    }
    pos[node.id] = { x, y: TOP + node.depth * VGAP };
    return x;
  };
  childrenOf(null).forEach(place);
  const total = cursor * SLOT;
  const editorW = editorOpen ? 320 : 0;
  const offset = Math.max(20, (canvasWidth - editorW - total) / 2);
  Object.values(pos).forEach((p) => {
    p.x += offset;
  });
  return pos;
}

function buildEdges(nodes: AgentNode[], pos: Record<string, NodePos>) {
  return nodes
    .filter((n) => n.parentId && pos[n.parentId] && pos[n.id])
    .map((n) => {
      const a = pos[n.parentId as string];
      const b = pos[n.id];
      const y1 = a.y + CARD_H;
      const y2 = b.y;
      const my = (y1 + y2) / 2;
      return { id: n.id, d: `M ${a.x} ${y1} C ${a.x} ${my}, ${b.x} ${my}, ${b.x} ${y2}` };
    });
}

const NODE_STATUS_COLOR: Record<Session['status'], string> = {
  running: 'var(--teal)',
  waiting: 'var(--dim)',
  reviewing: 'var(--amber)',
  done: 'var(--grn)',
};

export function TreeCanvas() {
  const { navigate, back, canGoBack } = useNavigation();
  const tree = useAgentTree();
  const {
    projects,
    selectedProjectId,
    setSelectedProjectId,
    selectedProject,
    nodes,
    sessions,
    accounts,
    rotation,
    projectsLoading,
    loading,
    applyingTemplate,
    roots,
    leaves,
    maxDepth,
    createNode,
    addChild,
    deleteNode,
    patchNode,
    clearTree,
    applyTemplate,
    handleRunTree,
  } = tree;

  const [selId, setSelId] = useState<string | null>(null);
  const [drag, setDrag] = useState<{ fromId: string; x: number; y: number } | null>(null);
  const [canvasRef, canvasWidth] = useElementWidth<HTMLDivElement>();
  const suppressNextCanvasClick = useRef(false);

  const editorOpen = selId !== null && nodes.some((n) => n.id === selId);
  const pos = useMemo(() => computeLayout(nodes, canvasWidth, editorOpen), [nodes, canvasWidth, editorOpen]);
  const edges = useMemo(() => buildEdges(nodes, pos), [nodes, pos]);
  const selectedNode = nodes.find((n) => n.id === selId) ?? null;

  let ghostPath: string | null = null;
  if (drag && pos[drag.fromId]) {
    const a = pos[drag.fromId];
    const y1 = a.y + CARD_H;
    const my = (y1 + drag.y) / 2;
    ghostPath = `M ${a.x} ${y1} C ${a.x} ${my}, ${drag.x} ${my}, ${drag.x} ${drag.y}`;
  }

  const canvasXY = (e: { clientX: number; clientY: number }) => {
    const r = canvasRef.current?.getBoundingClientRect();
    if (!r) return { x: 0, y: 0 };
    return { x: e.clientX - r.left, y: e.clientY - r.top };
  };

  const onPortDown = (nodeId: string) => (e: ReactMouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
    const node = nodes.find((n) => n.id === nodeId);
    if (!node || node.depth >= MAX_DEPTH) return;
    suppressNextCanvasClick.current = true;
    const c = canvasXY(e);
    setDrag({ fromId: nodeId, x: c.x, y: c.y });
  };

  const onCanvasMouseMove = (e: ReactMouseEvent) => {
    if (!drag) return;
    const c = canvasXY(e);
    setDrag((d) => (d ? { ...d, x: c.x, y: c.y } : d));
  };

  const onCanvasMouseUp = () => {
    if (!drag) return;
    const fromId = drag.fromId;
    setDrag(null);
    addChild(fromId).then((created) => {
      if (created) setSelId(created.id);
    });
  };

  // Safety net for a drop that lands outside the canvas element entirely
  // (fast pointer movement past the window edge) — cancels the ghost line
  // rather than guessing at a child creation the user may not have intended.
  useEffect(() => {
    if (!drag) return;
    const onWindowUp = () => setDrag(null);
    window.addEventListener('mouseup', onWindowUp);
    return () => window.removeEventListener('mouseup', onWindowUp);
  }, [drag]);

  const onCanvasClick = () => {
    if (suppressNextCanvasClick.current) {
      suppressNextCanvasClick.current = false;
      return;
    }
    setSelId(null);
  };

  const handleCreateRoot = () => {
    createNode(null, 'Orchestrator', 'Root agent — delegates to children.').then((created) => {
      if (created) setSelId(created.id);
    });
  };

  const handleApplyTemplate = (t: (typeof TEMPLATES)[number]) => {
    applyTemplate(t).then((rootId) => setSelId(rootId));
  };

  const handleDeleteNode = async (nodeId: string) => {
    await deleteNode(nodeId);
    if (selId === nodeId) setSelId(null);
  };

  const handleClearTree = async () => {
    await clearTree();
    setSelId(null);
  };

  return (
    <div className={styles.screen}>
      <div className={styles.header}>
        {canGoBack && (
          <Button variant="outline" className={styles.backBtn} onClick={back}>
            ← back
          </Button>
        )}
        <Button variant="outline" className={styles.backBtn} onClick={() => navigate('main-view')} title="Back to the main view">
          ⌂ main
        </Button>
        <span className={styles.badge}>⌥</span>
        <Select value={selectedProjectId ?? undefined} onValueChange={setSelectedProjectId}>
          <SelectTrigger className={styles.projectSelect} size="sm">
            <SelectValue placeholder="select a project…" />
          </SelectTrigger>
          <SelectContent>
            {projects.map((p) => (
              <SelectItem key={p.id} value={p.id}>
                {p.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <div className={styles.divider} />
        <span className={styles.templatesLabel}>TEMPLATES</span>
        {TEMPLATES.map((t) => {
          const chip = (
            <Button
              key={t.key}
              variant="outline"
              size="sm"
              disabled={!selectedProjectId || applyingTemplate !== null}
              onClick={nodes.length === 0 ? () => handleApplyTemplate(t) : undefined}
            >
              {applyingTemplate === t.key ? <Spinner label="" /> : t.name}
            </Button>
          );
          if (nodes.length === 0) return chip;
          return (
            <ConfirmDeleteDialog
              key={t.key}
              trigger={chip}
              title={`Replace the tree with "${t.name}"?`}
              description="This deletes every existing node in this project's tree and creates the template's nodes instead."
              confirmLabel="Replace"
              onConfirm={async () => {
                await clearTree();
                await handleApplyTemplate(t);
              }}
            />
          );
        })}
        <ConfirmDeleteDialog
          trigger={
            <Button variant="outline" size="sm" disabled={!selectedProjectId || nodes.length === 0}>
              clear
            </Button>
          }
          title="Clear the tree?"
          description="Deletes every node in this project's agent tree. This can't be undone."
          confirmLabel="Clear"
          onConfirm={handleClearTree}
        />
        <span className={styles.spacer} />
        <div className={styles.viewSwitcher}>
          <span className={styles.viewSwitcherActive}>canvas</span>
          <button className={styles.viewSwitcherLink} onClick={() => navigate('tree-columns', { projectId: selectedProjectId })}>
            columns
          </button>
          <button className={styles.viewSwitcherLink} onClick={() => navigate('tree-outline', { projectId: selectedProjectId })}>
            outline
          </button>
        </div>
        <span className={styles.nodeCountLabel}>
          {nodes.length} agent{nodes.length === 1 ? '' : 's'} · {leaves.length} leaf{leaves.length === 1 ? '' : 'es'}
        </span>
        <span className={`${styles.depthBadge} ${maxDepth >= MAX_DEPTH ? styles.depthBadgeMax : ''}`}>
          depth {maxDepth}/{MAX_DEPTH}
        </span>
        <Button
          disabled={roots.length !== 1}
          title={roots.length !== 1 ? 'A tree needs exactly one root node to run' : undefined}
          onClick={handleRunTree}
        >
          ▶ Run tree
        </Button>
      </div>

      <div className={styles.body}>
        <div ref={canvasRef} className={styles.canvas} onMouseMove={onCanvasMouseMove} onMouseUp={onCanvasMouseUp} onClick={onCanvasClick}>
          <svg className={styles.edgesSvg}>
            {edges.map((e) => (
              <path key={e.id} d={e.d} fill="none" stroke="var(--line2)" strokeWidth={1.5} />
            ))}
            {ghostPath && <path d={ghostPath} fill="none" stroke="var(--acc)" strokeWidth={1.5} strokeDasharray="5 4" />}
          </svg>

          {nodes.map((n) => {
            const p = pos[n.id];
            if (!p) return null;
            const account = accounts.find((a) => a.id === n.accountId) ?? null;
            const prov = account ? providerMeta(account.provider) : null;
            const session = sessions.find((s) => s.nodeId === n.id);
            const atMax = n.depth >= MAX_DEPTH;
            const isSel = n.id === selId;
            return (
              <div
                key={n.id}
                className={`${styles.nodeCard} ${isSel ? styles.nodeCardSelected : ''}`}
                style={{ left: p.x - CARD_W / 2, top: p.y, width: CARD_W }}
                onClick={(e) => {
                  e.stopPropagation();
                  setSelId(n.id);
                }}
              >
                <div className={styles.nodeHeader}>
                  <span
                    className={styles.nodeProvBadge}
                    style={{ background: prov?.tintVar ?? UNKNOWN_PROVIDER_META.tintVar, color: prov?.colorVar ?? UNKNOWN_PROVIDER_META.colorVar }}
                  >
                    {prov?.letter ?? UNKNOWN_PROVIDER_META.letter}
                  </span>
                  <span className={styles.nodeRole}>{n.role}</span>
                  <span className={styles.spacer} />
                  <span
                    className={styles.nodeStatusDot}
                    style={{ background: session ? NODE_STATUS_COLOR[session.status] : 'var(--line2)' }}
                    title={session ? `session: ${session.status}` : 'no session yet'}
                  />
                </div>
                <div className={styles.nodePromptPreview}>{n.prompt || '(no prompt set)'}</div>
                <div className={styles.nodeBadgesRow}>
                  <span className={`${styles.nodeBadge} ${styles.nodeBadgeModel}`}>{n.model}</span>
                  <span className={`${styles.nodeBadge} ${account ? styles.nodeBadgeAccount : styles.nodeBadgeUnassigned}`}>
                    {account?.name ?? 'unassigned'}
                  </span>
                </div>
                <div
                  className={`${styles.nodePort} ${atMax ? styles.nodePortDisabled : ''}`}
                  title={atMax ? `Max depth (${MAX_DEPTH}) reached` : 'Drag to create a child agent'}
                  onMouseDown={onPortDown(n.id)}
                />
              </div>
            );
          })}

          {projectsLoading && (
            <div className={styles.emptyState}>
              <Spinner label="loading projects…" />
            </div>
          )}

          {!projectsLoading && !selectedProjectId && (
            <div className={styles.emptyState}>
              <span className={styles.emptyStateText}>Create a project first.</span>
            </div>
          )}

          {!projectsLoading && selectedProjectId && loading && (
            <div className={styles.emptyState}>
              <Spinner label="loading…" />
            </div>
          )}

          {!projectsLoading && selectedProjectId && !loading && nodes.length === 0 && (
            <div className={styles.emptyState}>
              <span className={styles.emptyStateText}>No agent tree yet for {selectedProject?.name ?? 'this project'}.</span>
              <Button onClick={handleCreateRoot}>+ Create root agent</Button>
            </div>
          )}

          <div className={styles.hintBar}>
            <span>
              <span className={styles.hintDot}>●</span> drag a port ↓ to spawn a child
            </span>
            <span>click node → edit</span>
            <span>
              max depth {MAX_DEPTH}
            </span>
          </div>
        </div>

        {selectedNode && (
          <NodeEditorPanel
            key={selectedNode.id}
            node={selectedNode}
            depthLabel={`level ${selectedNode.depth}`}
            accounts={accounts}
            rotation={rotation}
            session={sessions.find((s) => s.nodeId === selectedNode.id)}
            atMaxDepth={selectedNode.depth >= MAX_DEPTH}
            onPatch={(fields) => patchNode(selectedNode.id, fields)}
            onAddChild={() => addChild(selectedNode.id).then((created) => created && setSelId(created.id))}
            onDelete={() => handleDeleteNode(selectedNode.id)}
            onClose={() => setSelId(null)}
            onOpenSession={(sessionId) => navigate('agent-panel', { sessionId })}
          />
        )}
      </div>
    </div>
  );
}

function NodeEditorPanel({
  node,
  depthLabel,
  accounts,
  rotation,
  session,
  atMaxDepth,
  onPatch,
  onAddChild,
  onDelete,
  onClose,
  onOpenSession,
}: {
  node: AgentNode;
  depthLabel: string;
  accounts: Account[];
  rotation: ProjectAccountRotation[];
  session: Session | undefined;
  atMaxDepth: boolean;
  onPatch: (fields: Omit<UpdateAgentNode, 'id'>) => Promise<AgentNode>;
  onAddChild: () => void;
  onDelete: () => Promise<void>;
  onClose: () => void;
  onOpenSession: (sessionId: string) => void;
}) {
  const {
    role,
    setRole,
    prompt,
    setPrompt,
    accountId,
    model,
    models,
    loadingModels,
    saving,
    error,
    selectableAccounts,
    persistRole,
    persistPrompt,
    handleAccountChange,
    handleModelChange,
  } = useNodeEditor(node, accounts, rotation, onPatch);

  return (
    <div className={styles.editorPanel}>
      <div className={styles.editorHeader}>
        <span className={styles.editorLabel}>AGENT NODE</span>
        <span className={styles.editorDepthBadge}>{depthLabel}</span>
        {saving && <Spinner label="" />}
        <span className={styles.spacer} />
        <button className={styles.editorClose} onClick={onClose}>
          ✕
        </button>
      </div>
      <div className={styles.editorBody}>
        <div className={styles.editorField}>
          <Label className={styles.editorFieldLabel}>ROLE</Label>
          <Input value={role} onChange={(e) => setRole(e.target.value)} onBlur={persistRole} />
        </div>
        <div className={styles.editorField}>
          <Label className={styles.editorFieldLabel}>PROMPT / INSTRUCTIONS</Label>
          <Textarea value={prompt} onChange={(e) => setPrompt(e.target.value)} onBlur={persistPrompt} rows={6} />
        </div>
        <div className={styles.editorField}>
          <Label className={styles.editorFieldLabel}>ACCOUNT</Label>
          <Select value={accountId || undefined} onValueChange={handleAccountChange}>
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
          <span className={styles.editorFieldHint}>Only accounts allowed for this project are listed (rotation settings).</span>
        </div>
        <div className={styles.editorField}>
          <Label className={styles.editorFieldLabel}>MODEL</Label>
          <Select value={model} onValueChange={handleModelChange} disabled={!accountId || loadingModels}>
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

        {session && (
          <button className={styles.editorSessionLink} onClick={() => onOpenSession(session.id)}>
            open session ({session.status}) →
          </button>
        )}

        {error && <span className={styles.editorError}>{error}</span>}

        <div className={styles.editorActions}>
          <Button variant="outline" disabled={atMaxDepth} style={{ flex: 1 }} onClick={onAddChild}>
            + Add child
          </Button>
          {node.parentId && (
            <ConfirmDeleteDialog
              trigger={
                <Button variant="destructive">Delete</Button>
              }
              title={`Delete "${node.role}"?`}
              description="Deletes this node and every node under it. This can't be undone."
              onConfirm={onDelete}
            />
          )}
        </div>
        <div className={styles.editorHint}>
          On run, this node opens as a panel; when its children finish, it receives their summaries for review before reporting upward.
        </div>
      </div>
    </div>
  );
}
