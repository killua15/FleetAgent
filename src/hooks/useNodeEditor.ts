import { useEffect, useRef, useState } from 'react';
import { invoke } from '../lib/invoke';
import type { Account } from '../types/account';
import type { AgentNode, UpdateAgentNode } from '../types/agentNode';
import type { ModelOption } from '../types/session';
import type { ProjectAccountRotation } from '../types/settings';

/** Role/prompt/model/account editing for one `AgentNode`, shared by every
 * tree-hierarchy screen's editor (side panel in `TreeCanvas`, bottom drawer
 * in `TreeColumns`, inline row in `TreeOutline`) — only the surrounding
 * markup differs per screen. The caller MUST mount its editor component
 * with `key={node.id}` so this hook's local state resets per node and its
 * flush-on-unmount effect fires exactly when the user switches away from
 * one node to another. */
export function useNodeEditor(node: AgentNode, accounts: Account[], rotation: ProjectAccountRotation[], onPatch: (fields: Omit<UpdateAgentNode, 'id'>) => Promise<AgentNode>) {
  const [role, setRole] = useState(node.role);
  const [prompt, setPrompt] = useState(node.prompt);
  const [accountId, setAccountId] = useState(node.accountId ?? '');
  const [model, setModel] = useState(node.model);
  const [models, setModels] = useState<ModelOption[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Flush-on-unmount catches role/prompt edits typed but not yet blurred
  // when the user switches to a different node. The "saved" refs (not the
  // `node` prop) are the dirty-check baseline because `node` can update in
  // place while this instance stays mounted (e.g. a background reload)
  // without reflecting what's actually been persisted.
  const roleRef = useRef(role);
  roleRef.current = role;
  const promptRef = useRef(prompt);
  promptRef.current = prompt;
  const accountIdRef = useRef(accountId);
  accountIdRef.current = accountId;
  const modelRef = useRef(model);
  modelRef.current = model;
  const savedRoleRef = useRef(node.role);
  const savedPromptRef = useRef(node.prompt);

  useEffect(() => {
    if (!accountId) return;
    setLoadingModels(true);
    invoke<ModelOption[]>('list_provider_models', { accountId })
      .then((list) => setModels(list))
      .catch(() => setModels([]))
      .finally(() => setLoadingModels(false));
    // Only for the initially-selected node's own account — user-driven
    // account changes are handled explicitly in `handleAccountChange` so the
    // model resolves and saves together, atomically.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    return () => {
      if (roleRef.current !== savedRoleRef.current || promptRef.current !== savedPromptRef.current) {
        onPatch({
          role: roleRef.current,
          prompt: promptRef.current,
          accountId: accountIdRef.current || null,
          model: modelRef.current,
        }).catch(() => {});
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const selectableAccounts = accounts.filter((a) => a.id === node.accountId || rotation.find((r) => r.accountId === a.id)?.allowed);

  const persist = async (overrides: Partial<Omit<UpdateAgentNode, 'id'>>) => {
    setSaving(true);
    setError(null);
    try {
      await onPatch({ role, prompt, accountId: accountId || null, model, ...overrides });
      // Every save round-trips the full role/prompt, so this is always a
      // valid new baseline for the flush-on-unmount dirty check above.
      savedRoleRef.current = roleRef.current;
      savedPromptRef.current = promptRef.current;
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const handleAccountChange = async (newAccountId: string) => {
    setAccountId(newAccountId);
    setLoadingModels(true);
    try {
      const list = await invoke<ModelOption[]>('list_provider_models', { accountId: newAccountId });
      setModels(list);
      const nextModel = list.some((m) => m.id === model) ? model : (list[0]?.id ?? model);
      setModel(nextModel);
      await persist({ accountId: newAccountId, model: nextModel });
    } catch (e) {
      setError(String(e));
    } finally {
      setLoadingModels(false);
    }
  };

  const handleModelChange = async (newModel: string) => {
    setModel(newModel);
    await persist({ model: newModel });
  };

  return {
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
    persistRole: () => persist({ role }),
    persistPrompt: () => persist({ prompt }),
    handleAccountChange,
    handleModelChange,
  };
}
