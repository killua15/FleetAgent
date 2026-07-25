import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { toastError } from '../components/ErrorOverlay';
import { useNavigation } from '../nav/NavigationProvider';
import type { Account } from '../types/account';
import type { AgentNode, NewAgentNode, UpdateAgentNode } from '../types/agentNode';
import type { Project, Session } from '../types/session';
import type { ProjectAccountRotation } from '../types/settings';

// Mirrors the `MAX_DEPTH`/CHECK constraint in `agent_nodes.rs`/schema.sql —
// duplicated here only for instant UI feedback (disabling "add child" before
// a round trip); the backend remains the real source of truth.
export const MAX_DEPTH = 3;

export const PLACEHOLDER_MODEL = 'sonnet';

export interface TemplateNodeDef {
  tempId: string;
  parentTempId: string | null;
  role: string;
  prompt: string;
}

export interface Template {
  key: string;
  name: string;
  nodes: TemplateNodeDef[];
}

// Preset trees a user can drop in as a starting point — account assignment
// is deliberately left for the editor afterward (the account choice is
// per-project/rotation-scoped, unknowable at template-definition time), so
// every node starts with `accountId: null` and a placeholder model that
// self-corrects the moment a real account is picked (see `useNodeEditor`'s
// `handleAccountChange`).
export const TEMPLATES: Template[] = [
  {
    key: 'dev',
    name: 'Dev company',
    nodes: [
      { tempId: 't1', parentTempId: null, role: 'Product Lead', prompt: 'Own the spec. Break work into tracks, accept/reject results.' },
      { tempId: 't2', parentTempId: 't1', role: 'Tech Lead', prompt: 'Turn spec into tasks. Review child diffs before they reach Product Lead.' },
      { tempId: 't3', parentTempId: 't2', role: 'Frontend', prompt: 'Implement UI tasks. Follow existing patterns.' },
      { tempId: 't4', parentTempId: 't2', role: 'Backend', prompt: 'API + persistence tasks. Keep migrations reversible.' },
      { tempId: 't5', parentTempId: 't2', role: 'QA', prompt: 'Write/run tests for every sibling diff. Report failures upward.' },
    ],
  },
  {
    key: 'pair',
    name: 'Pair + reviewer',
    nodes: [
      { tempId: 't1', parentTempId: null, role: 'Lead', prompt: 'Coordinate the pair. Split the task, review both outputs, merge.' },
      { tempId: 't2', parentTempId: 't1', role: 'Builder', prompt: 'Implement the feature end to end. Small commits.' },
      { tempId: 't3', parentTempId: 't1', role: 'Reviewer', prompt: 'Review Builder diffs. Flag risks, request changes.' },
    ],
  },
  {
    key: 'docs',
    name: 'Docs pipeline',
    nodes: [
      { tempId: 't1', parentTempId: null, role: 'Editor', prompt: 'Own the doc. Assemble sections, keep voice consistent.' },
      { tempId: 't2', parentTempId: 't1', role: 'Researcher', prompt: 'Gather sources and code references. Cite files.' },
      { tempId: 't3', parentTempId: 't1', role: 'Writer', prompt: 'Draft sections from research notes.' },
      { tempId: 't4', parentTempId: 't3', role: 'Proofreader', prompt: 'Check drafts for accuracy against the codebase.' },
    ],
  },
];

/** All the data-loading + CRUD a tree-hierarchy screen needs, shared across
 * `TreeCanvas`/`TreeColumns`/`TreeOutline` (identical for all three — only
 * the layout/selection differs per screen, which stays local to each). */
export function useAgentTree() {
  const { current, navigate } = useNavigation();
  // Honors a `projectId` handed in via navigation (e.g. "edit tree" from
  // `exec-run`, or switching view mode from a sibling tree screen) so it
  // opens the right project instead of always defaulting to the first one.
  const incomingProjectId = current.params?.projectId as string | undefined;
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(incomingProjectId ?? null);
  const [nodes, setNodes] = useState<AgentNode[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [rotation, setRotation] = useState<ProjectAccountRotation[]>([]);
  // Separate from `loading` below: this only covers the initial project
  // list fetch. `loading` used to double as "nothing to show yet" for the
  // no-projects case too, but it's only ever cleared inside `loadProjectData`
  // — which never runs without a selected project — so a project-less
  // workspace got stuck on the spinner forever instead of reaching the
  // "create a project first" empty state.
  const [projectsLoading, setProjectsLoading] = useState(true);
  const [loading, setLoading] = useState(false);
  const [applyingTemplate, setApplyingTemplate] = useState<string | null>(null);

  useEffect(() => {
    invoke<Project[]>('list_projects')
      .then((list) => {
        setProjects(list);
        setSelectedProjectId((prev) => prev ?? incomingProjectId ?? list[0]?.id ?? null);
      })
      .catch((e) => toastError(String(e)))
      .finally(() => setProjectsLoading(false));
    invoke<Account[]>('list_accounts')
      .then(setAccounts)
      .catch((e) => toastError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadProjectData = async (projectId: string) => {
    setLoading(true);
    try {
      const [nodeList, sessionList, rot] = await Promise.all([
        invoke<AgentNode[]>('list_agent_nodes', { projectId }),
        invoke<Session[]>('list_project_sessions', { projectId }),
        invoke<ProjectAccountRotation[]>('list_project_account_rotation', { projectId }),
      ]);
      setNodes(nodeList);
      setSessions(sessionList);
      setRotation(rot);
      return { nodes: nodeList, sessions: sessionList };
    } catch (e) {
      toastError(String(e));
      return { nodes, sessions };
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!selectedProjectId) return;
    loadProjectData(selectedProjectId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedProjectId]);

  const roots = nodes.filter((n) => n.parentId === null);
  const leaves = nodes.filter((n) => !nodes.some((k) => k.parentId === n.id));
  const maxDepth = nodes.reduce((m, n) => Math.max(m, n.depth), 0);
  const selectedProject = projects.find((p) => p.id === selectedProjectId) ?? null;

  const patchNode = async (nodeId: string, fields: Omit<UpdateAgentNode, 'id'>) => {
    const updated = await invoke<AgentNode>('update_agent_node', { input: { id: nodeId, ...fields } });
    setNodes((prev) => prev.map((n) => (n.id === updated.id ? updated : n)));
    return updated;
  };

  const createNode = async (parentId: string | null, role: string, prompt: string): Promise<AgentNode | null> => {
    if (!selectedProjectId) return null;
    const parent = parentId ? nodes.find((n) => n.id === parentId) : null;
    try {
      const created = await invoke<AgentNode>('create_agent_node', {
        input: {
          projectId: selectedProjectId,
          parentId,
          role,
          prompt,
          model: parent?.model ?? PLACEHOLDER_MODEL,
          accountId: null,
        } satisfies NewAgentNode,
      });
      setNodes((prev) => [...prev, created]);
      return created;
    } catch (e) {
      toastError(String(e));
      return null;
    }
  };

  const addChild = (parentId: string): Promise<AgentNode | null> => {
    const parent = nodes.find((n) => n.id === parentId);
    if (!parent || parent.depth >= MAX_DEPTH) return Promise.resolve(null);
    return createNode(parentId, 'New agent', 'Describe this agent’s task…');
  };

  const deleteNode = async (nodeId: string) => {
    await invoke('delete_agent_node', { id: nodeId });
    setNodes((prev) => prev.filter((n) => n.id !== nodeId && n.parentId !== nodeId));
    if (selectedProjectId) await loadProjectData(selectedProjectId);
  };

  const clearTree = async () => {
    await Promise.all(roots.map((r) => invoke('delete_agent_node', { id: r.id })));
    if (selectedProjectId) await loadProjectData(selectedProjectId);
  };

  const applyTemplate = async (template: Template): Promise<string | null> => {
    if (!selectedProjectId) return null;
    setApplyingTemplate(template.key);
    try {
      const idMap: Record<string, string> = {};
      for (const def of template.nodes) {
        const created = await invoke<AgentNode>('create_agent_node', {
          input: {
            projectId: selectedProjectId,
            parentId: def.parentTempId ? idMap[def.parentTempId] : null,
            role: def.role,
            prompt: def.prompt,
            model: PLACEHOLDER_MODEL,
            accountId: null,
          } satisfies NewAgentNode,
        });
        idMap[def.tempId] = created.id;
      }
      await loadProjectData(selectedProjectId);
      return idMap.t1 ?? null;
    } catch (e) {
      toastError(String(e));
      return null;
    } finally {
      setApplyingTemplate(null);
    }
  };

  const handleRunTree = () => {
    if (roots.length !== 1 || !selectedProjectId) return;
    const root = roots[0];
    // `run_agent_tree` blocks server-side until the whole recursive tree is
    // done (each node waits on its children first — can take many minutes).
    // Tauri commands keep running regardless of whether the caller awaits
    // them, so this fires it and hands off to `exec-run` immediately for
    // live per-node progress instead of freezing this screen behind one
    // spinner for the entire run. A hard failure (e.g. a node with no
    // account assigned) still surfaces via the global toast wherever the
    // user ends up.
    invoke('run_agent_tree', { rootId: root.id }).catch((e) => toastError(String(e)));
    navigate('exec-run', { projectId: selectedProjectId, rootId: root.id });
  };

  return {
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
  };
}
