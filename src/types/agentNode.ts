/** Mirrors `AgentNode` in `src-tauri/src/models.rs` — a role/prompt/model/
 * account template in a tree-canvas hierarchy, up to depth 3 via `parentId`. */
export interface AgentNode {
  id: string;
  projectId: string;
  parentId: string | null;
  role: string;
  prompt: string;
  model: string;
  accountId: string | null;
  depth: number;
  createdAt: string;
}

export interface NewAgentNode {
  projectId: string;
  parentId: string | null;
  role: string;
  prompt: string;
  model: string;
  accountId: string | null;
}

export interface UpdateAgentNode {
  id: string;
  role: string;
  prompt: string;
  model: string;
  accountId: string | null;
}
