import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { SettingsTabs } from '../components/SettingsTabs';
import { toastError } from '../components/ErrorOverlay';
import { Button } from '../components/ui/button';
import { KNOWLEDGE_SOURCE_META, type KnowledgeSource, type KnowledgeSourceKind } from '../types/knowledge';
import type { Project } from '../types/session';
import shared from './TaskSettings.module.css';
import styles from './DocsSettings.module.css';

interface Form {
  id: string | null;
  kind: KnowledgeSourceKind;
  label: string;
  baseUrl: string;
  token: string;
  allProjects: boolean;
  projectIds: string[];
}

/**
 * Where your team keeps what it knows — an Outline wiki, or any docs tool with
 * an MCP server — handed to every agent Hangar opens, so they search and cite
 * the docs instead of you pasting them in.
 *
 * Laid out like Settings → Tasks on purpose: a source is an account on a tool,
 * checked against the tool before it's stored, with the secret in the keychain.
 */
export function DocsSettings() {
  const [sources, setSources] = useState<KnowledgeSource[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [form, setForm] = useState<Form | null>(null);
  const [saving, setSaving] = useState(false);

  const load = () => invoke<KnowledgeSource[]>('list_knowledge_sources').then(setSources);

  useEffect(() => {
    Promise.all([load(), invoke<Project[]>('list_projects').then(setProjects)]).catch((e) => toastError(String(e)));
  }, []);

  const visibleProjects = projects.filter((p) => !p.hidden);
  const projectName = (id: string) => projects.find((p) => p.id === id)?.name ?? 'a removed project';

  const startAdd = (kind: KnowledgeSourceKind) => setForm({ id: null, kind, label: '', baseUrl: '', token: '', allProjects: true, projectIds: [] });
  const startEdit = (s: KnowledgeSource) =>
    setForm({ id: s.id, kind: s.kind, label: s.label, baseUrl: s.baseUrl, token: '', allProjects: s.allProjects, projectIds: s.projectIds });

  const save = async () => {
    if (!form) return;
    setSaving(true);
    try {
      await invoke<KnowledgeSource>('save_knowledge_source', {
        id: form.id,
        kind: form.kind,
        label: form.label,
        baseUrl: form.baseUrl,
        token: form.token || null,
        allProjects: form.allProjects,
        projectIds: form.allProjects ? [] : form.projectIds,
      });
      setForm(null);
      await load();
    } catch (e) {
      // Next to the form that can fix it: the message names what was wrong
      // (the address, the key, the network).
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (s: KnowledgeSource) => {
    try {
      await invoke('delete_knowledge_source', { sourceId: s.id });
      await load();
    } catch (e) {
      toastError(String(e));
    }
  };

  const toggleProject = (id: string) =>
    setForm((f) => f && { ...f, projectIds: f.projectIds.includes(id) ? f.projectIds.filter((p) => p !== id) : [...f.projectIds, id] });

  const meta = form ? KNOWLEDGE_SOURCE_META[form.kind] : null;

  return (
    <div className={shared.screen}>
      <SettingsTabs active="docs-settings" title="Docs" subtitle="your team's wiki, in every agent" />

      <div className={shared.body}>
        <section className={shared.section}>
          <span className={shared.sectionTitle}>Knowledge sources</span>
          {sources.length === 0 && (
            <span className={shared.empty}>
              None yet. Connect your team's wiki and every agent you open here can search it, cite it and write to it — through the wiki's own MCP
              server.
            </span>
          )}
          {sources.map((s) => (
            <div key={s.id} className={shared.sourceRow}>
              <span className={shared.sourceName}>{s.label}</span>
              <span className={shared.sourceKind}>{KNOWLEDGE_SOURCE_META[s.kind]?.name ?? s.kind}</span>
              {typeof s.config.user === 'string' && <span className={shared.sourceLogin}>as {s.config.user}</span>}
              <span className={styles.mcpUrl} title="Where agents reach it">
                {s.mcpUrl}
              </span>
              {s.hasCredential ? (
                <span className={shared.tokenTag}>token stored</span>
              ) : (
                KNOWLEDGE_SOURCE_META[s.kind]?.tokenRequired && <span className={shared.tokenMissing}>token unreadable — enter it again</span>
              )}
              <span className={styles.scope}>
                {s.allProjects ? 'all projects' : s.projectIds.length === 0 ? 'no project' : s.projectIds.map(projectName).join(', ')}
              </span>
              <span className={shared.spacer} />
              <button className={shared.removeLink} onClick={() => startEdit(s)}>
                edit
              </button>
              <button className={shared.removeLink} onClick={() => remove(s)}>
                remove
              </button>
            </div>
          ))}

          {form === null || !meta ? (
            <div className={styles.providers}>
              {(Object.keys(KNOWLEDGE_SOURCE_META) as KnowledgeSourceKind[]).map((kind) => (
                <ProviderCard key={kind} kind={kind} onConnect={() => startAdd(kind)} />
              ))}
            </div>
          ) : (
            <div className={shared.form}>
              <span className={shared.formTitle}>{meta.name}</span>
              <span className={shared.hint}>{meta.hint}</span>
              <HowItIntegrates kind={form.kind} />
              <div className={shared.formRow}>
                <input className={shared.input} placeholder="name it — e.g. Team docs" value={form.label} onChange={(e) => setForm({ ...form, label: e.target.value })} />
                <input
                  className={shared.input}
                  placeholder={meta.urlPlaceholder}
                  aria-label={meta.urlLabel}
                  value={form.baseUrl}
                  onChange={(e) => setForm({ ...form, baseUrl: e.target.value })}
                />
                <input
                  className={shared.input}
                  type="password"
                  placeholder={form.id ? 'leave blank to keep the stored one' : meta.tokenPlaceholder}
                  value={form.token}
                  onChange={(e) => setForm({ ...form, token: e.target.value })}
                />
              </div>
              <label className={styles.check}>
                <input type="checkbox" checked={form.allProjects} onChange={(e) => setForm({ ...form, allProjects: e.target.checked })} />
                Every project's agents get it
              </label>
              {!form.allProjects && (
                <div className={styles.projects}>
                  {visibleProjects.map((p) => (
                    <label key={p.id} className={styles.check}>
                      <input type="checkbox" checked={form.projectIds.includes(p.id)} onChange={() => toggleProject(p.id)} />
                      {p.name}
                    </label>
                  ))}
                </div>
              )}
              <span className={shared.hint}>
                The token goes to the macOS keychain, never to the database. Agents get it as an environment variable — never on the command line
                or in a file — and it's checked against the tool before it's saved. Agents opened after this get it; restart an open terminal to
                give it to that one.
              </span>
              <div className={shared.formActions}>
                <Button size="sm" variant="outline" onClick={() => setForm(null)}>
                  cancel
                </Button>
                <Button size="sm" disabled={saving || !form.baseUrl.trim()} onClick={save}>
                  {saving ? 'checking…' : form.id ? 'Save' : 'Connect'}
                </Button>
              </div>
            </div>
          )}
        </section>
      </div>
    </div>
  );
}

/** One provider before it's connected: what it is, how it plugs in, and what
 * agents get — so the decision is made before a key is pasted. */
function ProviderCard({ kind, onConnect }: { kind: KnowledgeSourceKind; onConnect: () => void }) {
  const meta = KNOWLEDGE_SOURCE_META[kind];
  return (
    <div className={styles.card}>
      <div className={styles.cardHead}>
        <span className={styles.cardName}>{meta.name}</span>
        <span className={shared.spacer} />
        <Button size="sm" variant="outline" onClick={onConnect}>
          + Connect
        </Button>
      </div>
      <span className={styles.cardSummary}>{meta.summary}</span>
      <HowItIntegrates kind={kind} />
    </div>
  );
}

function HowItIntegrates({ kind }: { kind: KnowledgeSourceKind }) {
  const meta = KNOWLEDGE_SOURCE_META[kind];
  return (
    <details className={styles.how}>
      <summary className={styles.howHead}>How it integrates</summary>
      <ol className={styles.steps}>
        {meta.steps.map((step) => (
          <li key={step}>{step}</li>
        ))}
      </ol>
      <span className={styles.gets}>
        <strong>Agents get:</strong> {meta.agentsGet}
      </span>
      {meta.limits && (
        <span className={styles.limits}>
          <strong>Not yet:</strong> {meta.limits}
        </span>
      )}
    </details>
  );
}
