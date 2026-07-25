import { useState } from 'react';
import { Button } from '../components/ui/button';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '../components/ui/select';
import { Input } from '../components/ui/input';
import { Textarea } from '../components/ui/textarea';
import { Label } from '../components/ui/label';
import { Spinner } from '../components/Spinner';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import { useAgentTree, MAX_DEPTH, TEMPLATES, type Template } from '../hooks/useAgentTree';
import { useNodeEditor } from '../hooks/useNodeEditor';
import { useNavigation } from '../nav/NavigationProvider';
import { PROVIDER_META, type Account } from '../types/account';
import type { AgentNode, UpdateAgentNode } from '../types/agentNode';
import type { Session } from '../types/session';
import type { ProjectAccountRotation } from '../types/settings';
import styles from './TreeOutline.module.css';

const INDENT_PX = 26;

function orderDepthFirst(nodes: AgentNode[]): AgentNode[] {
  const ordered: AgentNode[] = [];
  const childrenOf = (id: string | null) => nodes.filter((n) => n.parentId === id);
  const walk = (id: string) => {
    childrenOf(id).forEach((n) => {
      ordered.push(n);
      walk(n.id);
    });
  };
  childrenOf(null).forEach((r) => {
    ordered.push(r);
    walk(r.id);
  });
  return ordered;
}

export function TreeOutline() {
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

  const [expandedId, setExpandedId] = useState<string | null>(null);

  const ordered = orderDepthFirst(nodes);

  const handleCreateRoot = () => {
    createNode(null, 'Orchestrator', 'Root agent — delegates to children.').then((created) => {
      if (created) setExpandedId(created.id);
    });
  };

  const handleAddChild = (parentId: string) => {
    addChild(parentId).then((created) => {
      if (created) setExpandedId(created.id);
    });
  };

  const handleApplyTemplate = (t: Template) => {
    applyTemplate(t).then((rootId) => setExpandedId(rootId));
  };

  const handleDeleteNode = async (nodeId: string) => {
    await deleteNode(nodeId);
    if (expandedId === nodeId) setExpandedId(null);
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
          <button className={styles.viewSwitcherLink} onClick={() => navigate('tree-columns', { projectId: selectedProjectId })}>
            columns
          </button>
          <span className={styles.viewSwitcherActive}>outline</span>
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

      <div className={styles.columnHeader}>
        <span>AGENT / PROMPT</span>
        <span>MODEL</span>
        <span>ACCOUNT</span>
        <span>REPORTS TO</span>
        <span style={{ textAlign: 'right' }}>ACTIONS</span>
      </div>

      <div className={styles.body}>
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

        {ordered.map((n) => {
          const account = accounts.find((a) => a.id === n.accountId) ?? null;
          const prov = account ? PROVIDER_META[account.provider] : null;
          const parent = n.parentId ? nodes.find((p) => p.id === n.parentId) : null;
          const isExpanded = expandedId === n.id;
          const canAdd = n.depth < MAX_DEPTH;
          return (
            <div key={n.id} className={`${styles.rowWrap} ${isExpanded ? styles.rowWrapSelected : ''}`}>
              <button className={styles.row} onClick={() => setExpandedId(isExpanded ? null : n.id)}>
                <div className={styles.rowMain}>
                  <span className={styles.indent} style={{ width: n.depth * INDENT_PX }} />
                  {n.depth > 0 && <span className={styles.connector}>↳</span>}
                  <span className={styles.provBadge} style={{ background: prov?.tintVar ?? 'var(--panel2)', color: prov?.colorVar ?? 'var(--dim)' }}>
                    {prov?.letter ?? '?'}
                  </span>
                  <span className={styles.role}>{n.role}</span>
                  <span className={styles.promptPreview}>— {n.prompt || '(no prompt set)'}</span>
                </div>
                <span className={styles.model}>{n.model}</span>
                <span className={styles.account}>{account?.name ?? 'unassigned'}</span>
                <span className={styles.reportsTo}>{parent?.role ?? '—'}</span>
                <div className={styles.actions}>
                  {canAdd && (
                    <span
                      role="button"
                      className={styles.actionBtn}
                      title="Add child"
                      onClick={(e) => {
                        e.stopPropagation();
                        handleAddChild(n.id);
                      }}
                    >
                      +
                    </span>
                  )}
                  {n.parentId && (
                    <ConfirmDeleteDialog
                      trigger={
                        <span
                          role="button"
                          className={`${styles.actionBtn} ${styles.deleteBtn}`}
                          title="Delete subtree"
                          onClick={(e) => e.stopPropagation()}
                        >
                          ✕
                        </span>
                      }
                      title={`Delete "${n.role}"?`}
                      description="Deletes this node and every node under it. This can't be undone."
                      onConfirm={() => handleDeleteNode(n.id)}
                    />
                  )}
                </div>
              </button>

              {isExpanded && (
                <OutlineRowEditor
                  key={n.id}
                  node={n}
                  accounts={accounts}
                  rotation={rotation}
                  session={sessions.find((s) => s.nodeId === n.id)}
                  onPatch={(fields) => patchNode(n.id, fields)}
                  onOpenSession={(sessionId) => navigate('agent-panel', { sessionId })}
                />
              )}
            </div>
          );
        })}
      </div>

      <div className={styles.footer}>
        <span>click a row to expand &amp; edit inline</span>
        <span>·</span>
        <span>+ adds a child (max depth {MAX_DEPTH})</span>
        <span>·</span>
        <span>✕ deletes the subtree</span>
        <span className={styles.spacer} />
        <span className={`${styles.depthBadge} ${maxDepth >= MAX_DEPTH ? styles.depthBadgeMax : ''}`}>
          depth {maxDepth}/{MAX_DEPTH}
        </span>
      </div>
    </div>
  );
}

function OutlineRowEditor({
  node,
  accounts,
  rotation,
  session,
  onPatch,
  onOpenSession,
}: {
  node: AgentNode;
  accounts: Account[];
  rotation: ProjectAccountRotation[];
  session: Session | undefined;
  onPatch: (fields: Omit<UpdateAgentNode, 'id'>) => Promise<AgentNode>;
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
    error,
    selectableAccounts,
    persistRole,
    persistPrompt,
    handleAccountChange,
    handleModelChange,
  } = useNodeEditor(node, accounts, rotation, onPatch);

  return (
    <div className={styles.editRow}>
      <div className={styles.editFields}>
        <div className={styles.editCol}>
          <Label className={styles.editFieldLabel}>ROLE</Label>
          <Input value={role} onChange={(e) => setRole(e.target.value)} onBlur={persistRole} />
        </div>
        <div className={styles.editCol}>
          <Label className={styles.editFieldLabel}>MODEL</Label>
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
        <div className={styles.editCol}>
          <Label className={styles.editFieldLabel}>ACCOUNT</Label>
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
        </div>
      </div>
      <div className={styles.editPromptWrap}>
        <Label className={styles.editFieldLabel}>PROMPT / INSTRUCTIONS</Label>
        <Textarea value={prompt} onChange={(e) => setPrompt(e.target.value)} onBlur={persistPrompt} rows={3} />
      </div>
      {session && (
        <button className={styles.editSessionLink} onClick={() => onOpenSession(session.id)}>
          open session ({session.status}) →
        </button>
      )}
      {error && <div className={styles.editError}>{error}</div>}
    </div>
  );
}
