import { useState } from 'react';
import { Button } from '../components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Input } from '../components/ui/input';
import { Textarea } from '../components/ui/textarea';
import { Label } from '../components/ui/label';
import { Spinner } from '../components/Spinner';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import { useAgentTree, MAX_DEPTH, TEMPLATES, type Template } from '../hooks/useAgentTree';
import { useElementWidth } from '../hooks/useElementWidth';
import { useNodeEditor } from '../hooks/useNodeEditor';
import { useNavigation } from '../nav/NavigationProvider';
import { providerMeta, UNKNOWN_PROVIDER_META, type Account } from '../types/account';
import type { AgentNode, UpdateAgentNode } from '../types/agentNode';
import type { Session } from '../types/session';
import type { ProjectAccountRotation } from '../types/settings';
import styles from './TreeColumns.module.css';

const CARD_W = 300;
const CARD_H = 92;
const SLOT = 130;
const TOP = 50;
const LANE_MARGIN = 24;
const LANE_GAP_MIN = 360;

const LANE_TITLES = ['LEVEL 1 — ORCHESTRATORS', 'LEVEL 2 — LEADS', 'LEVEL 3 — WORKERS'];

type NodePos = { x: number; y: number };

/** Transposed leaf-centering layout — same idea as `TreeCanvas`'s
 * `computeLayout`, just x = depth (lane) and y = the centered axis instead
 * of the other way around. */
function computeColumnsLayout(nodes: AgentNode[], containerWidth: number): { pos: Record<string, NodePos>; laneX: number[]; height: number } {
  const laneGap = Math.max(LANE_GAP_MIN, (containerWidth - LANE_MARGIN * 2 - CARD_W) / 2);
  const laneX = [LANE_MARGIN, LANE_MARGIN + laneGap, LANE_MARGIN + laneGap * 2];
  const childrenOf = (id: string | null) => nodes.filter((n) => n.parentId === id);
  const pos: Record<string, NodePos> = {};
  let cursor = 0;
  const place = (node: AgentNode): number => {
    const kids = childrenOf(node.id);
    let y: number;
    if (kids.length === 0 || node.depth >= MAX_DEPTH) {
      y = cursor * SLOT;
      cursor++;
    } else {
      const ys = kids.map(place);
      y = (ys[0] + ys[ys.length - 1]) / 2;
    }
    pos[node.id] = { x: laneX[node.depth] ?? laneX[laneX.length - 1], y: TOP + y };
    return y;
  };
  childrenOf(null).forEach(place);
  const height = Math.max(700, TOP + cursor * SLOT + 80);
  return { pos, laneX, height };
}

function buildEdges(nodes: AgentNode[], pos: Record<string, NodePos>) {
  return nodes
    .filter((n) => n.parentId && pos[n.parentId] && pos[n.id])
    .map((n) => {
      const a = pos[n.parentId as string];
      const b = pos[n.id];
      const x1 = a.x + CARD_W;
      const x2 = b.x;
      const y1 = a.y + CARD_H / 2;
      const y2 = b.y + CARD_H / 2;
      const mx = (x1 + x2) / 2;
      return { id: n.id, d: `M ${x1} ${y1} C ${mx} ${y1}, ${mx} ${y2}, ${x2} ${y2}` };
    });
}

export function TreeColumns() {
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
    createNode,
    addChild,
    deleteNode,
    patchNode,
    clearTree,
    applyTemplate,
    handleRunTree,
  } = tree;

  const [selId, setSelId] = useState<string | null>(null);
  const [containerRef, containerWidth] = useElementWidth<HTMLDivElement>();

  const { pos, laneX, height } = computeColumnsLayout(nodes, containerWidth);
  const edges = buildEdges(nodes, pos);
  const selectedNode = nodes.find((n) => n.id === selId) ?? null;
  const laneCounts = [0, 1, 2].map((d) => nodes.filter((n) => n.depth === d).length);

  const handleCreateRoot = () => {
    createNode(null, 'Orchestrator', 'Root agent — delegates to children.').then((created) => {
      if (created) setSelId(created.id);
    });
  };

  const handleAddChild = (parentId: string) => {
    addChild(parentId).then((created) => {
      if (created) setSelId(created.id);
    });
  };

  const handleApplyTemplate = (t: Template) => {
    applyTemplate(t).then((rootId) => setSelId(rootId));
  };

  const handleDeleteNode = async (nodeId: string) => {
    await deleteNode(nodeId);
    if (selId === nodeId) setSelId(null);
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
        <span className={styles.spacer} />
        <div className={styles.viewSwitcher}>
          <button className={styles.viewSwitcherLink} onClick={() => navigate('tree-canvas', { projectId: selectedProjectId })}>
            canvas
          </button>
          <span className={styles.viewSwitcherActive}>columns</span>
          <button className={styles.viewSwitcherLink} onClick={() => navigate('tree-outline', { projectId: selectedProjectId })}>
            outline
          </button>
        </div>
        <span className={styles.nodeCountLabel}>
          {nodes.length} agent{nodes.length === 1 ? '' : 's'} · {leaves.length} leaf{leaves.length === 1 ? '' : 'es'}
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
        <div ref={containerRef} className={styles.lanes}>
          {laneX.map((x, i) => (
            <div key={i} className={styles.laneHeader} style={{ left: x, top: 18, width: CARD_W }}>
              <span className={styles.laneTitle}>{LANE_TITLES[i]}</span>
              <span className={styles.laneCount}>
                {laneCounts[i]} agent{laneCounts[i] === 1 ? '' : 's'}
              </span>
            </div>
          ))}

          <svg className={styles.edgesSvg} width={containerWidth} height={height}>
            {edges.map((e) => (
              <path key={e.id} d={e.d} fill="none" stroke="var(--line2)" strokeWidth={1.5} />
            ))}
          </svg>

          {nodes.map((n) => {
            const p = pos[n.id];
            if (!p) return null;
            const account = accounts.find((a) => a.id === n.accountId) ?? null;
            const prov = account ? providerMeta(account.provider) : null;
            const isSel = n.id === selId;
            const canAdd = n.depth < MAX_DEPTH;
            return (
              <div
                key={n.id}
                className={`${styles.nodeCard} ${isSel ? styles.nodeCardSelected : ''}`}
                style={{ left: p.x, top: p.y, width: CARD_W }}
                onClick={() => setSelId(n.id)}
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
                  {canAdd && (
                    <button
                      className={styles.addChildBtn}
                      title="Add child in next lane"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleAddChild(n.id);
                      }}
                    >
                      +
                    </button>
                  )}
                </div>
                <div className={styles.nodePromptPreview}>{n.prompt || '(no prompt set)'}</div>
                <div className={styles.nodeBadgesRow}>
                  <span className={`${styles.nodeBadge} ${styles.nodeBadgeModel}`}>{n.model}</span>
                  <span className={`${styles.nodeBadge} ${account ? styles.nodeBadgeAccount : styles.nodeBadgeUnassigned}`}>
                    {account?.name ?? 'unassigned'}
                  </span>
                </div>
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
        </div>

        {selectedNode && (
          <NodeDrawer
            key={selectedNode.id}
            node={selectedNode}
            depthLabel={`level ${selectedNode.depth}`}
            accounts={accounts}
            rotation={rotation}
            session={sessions.find((s) => s.nodeId === selectedNode.id)}
            onPatch={(fields) => patchNode(selectedNode.id, fields)}
            onDelete={() => handleDeleteNode(selectedNode.id)}
            onClose={() => setSelId(null)}
            onOpenSession={(sessionId) => navigate('agent-panel', { sessionId })}
          />
        )}
      </div>
    </div>
  );
}

function NodeDrawer({
  node,
  depthLabel,
  accounts,
  rotation,
  session,
  onPatch,
  onDelete,
  onClose,
  onOpenSession,
}: {
  node: AgentNode;
  depthLabel: string;
  accounts: Account[];
  rotation: ProjectAccountRotation[];
  session: Session | undefined;
  onPatch: (fields: Omit<UpdateAgentNode, 'id'>) => Promise<AgentNode>;
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
    <div className={styles.drawer}>
      <div className={styles.drawerHeader}>
        <span className={styles.drawerLabel}>EDIT NODE</span>
        <span className={styles.drawerDepthBadge}>{depthLabel}</span>
        {saving && <Spinner label="" />}
        <span className={styles.spacer} />
        {session && (
          <button className={styles.editorSessionLink} onClick={() => onOpenSession(session.id)}>
            open session ({session.status}) →
          </button>
        )}
        {node.parentId && (
          <ConfirmDeleteDialog
            trigger={
              <Button variant="destructive" size="sm">
                Delete subtree
              </Button>
            }
            title={`Delete "${node.role}"?`}
            description="Deletes this node and every node under it. This can't be undone."
            onConfirm={onDelete}
          />
        )}
        <button className={styles.drawerClose} onClick={onClose}>
          ✕
        </button>
      </div>
      <div className={styles.drawerBody}>
        <div className={styles.drawerCol}>
          <Label className={styles.drawerFieldLabel}>ROLE</Label>
          <Input value={role} onChange={(e) => setRole(e.target.value)} onBlur={persistRole} />
          <Label className={styles.drawerFieldLabel} style={{ marginTop: 6 }}>
            MODEL
          </Label>
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
        <div className={styles.drawerCol}>
          <Label className={styles.drawerFieldLabel}>ACCOUNT</Label>
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
          <span className={styles.drawerHint}>Parent reviews this node's summary when it finishes.</span>
          {error && <span className={styles.editorError}>{error}</span>}
        </div>
        <div className={styles.drawerColWide}>
          <Label className={styles.drawerFieldLabel}>PROMPT / INSTRUCTIONS</Label>
          <Textarea
            className={styles.drawerPrompt}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onBlur={persistPrompt}
          />
        </div>
      </div>
    </div>
  );
}
