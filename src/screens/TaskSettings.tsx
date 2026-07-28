import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { SettingsTabs } from '../components/SettingsTabs';
import { toastError } from '../components/ErrorOverlay';
import { Button } from '../components/ui/button';
import { TASK_SOURCE_META, type TaskSource, type TaskSourceKind, type TaskSourceLink } from '../types/task';
import type { Project } from '../types/session';
import styles from './TaskSettings.module.css';

/**
 * Where the ☰ Tasks panel gets its rows.
 *
 * Two steps, because they're two different decisions: a **source** is an
 * account on a tool (one ClickUp token covers every workspace you're in), and a
 * **link** says which project that source feeds. The same ClickUp source can
 * feed four repos; a fifth repo takes its issues from GitHub instead.
 */
export function TaskSettings() {
  const [sources, setSources] = useState<TaskSource[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [projectId, setProjectId] = useState('');
  const [links, setLinks] = useState<TaskSourceLink[]>([]);
  const [adding, setAdding] = useState<TaskSourceKind | null>(null);
  const [token, setToken] = useState('');
  const [label, setLabel] = useState('');
  const [saving, setSaving] = useState(false);

  const loadSources = () => invoke<TaskSource[]>('list_task_sources').then(setSources);

  useEffect(() => {
    Promise.all([loadSources(), invoke<Project[]>('list_projects').then(setProjects)]).catch((e) => toastError(String(e)));
  }, []);

  useEffect(() => {
    if (!projectId) {
      setLinks([]);
      return;
    }
    invoke<TaskSourceLink[]>('list_task_source_links', { projectId }).then(setLinks).catch(() => setLinks([]));
  }, [projectId]);

  const save = async (kind: TaskSourceKind) => {
    setSaving(true);
    try {
      await invoke<TaskSource>('save_task_source', {
        id: null,
        kind,
        label: label.trim() || TASK_SOURCE_META[kind].name,
        token: token.trim() || null,
      });
      await loadSources();
      setAdding(null);
      setToken('');
      setLabel('');
    } catch (e) {
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (source: TaskSource) => {
    try {
      await invoke('delete_task_source', { sourceId: source.id });
      await loadSources();
      if (projectId) setLinks(await invoke<TaskSourceLink[]>('list_task_source_links', { projectId }));
    } catch (e) {
      toastError(String(e));
    }
  };

  const linkFor = (sourceId: string) => links.find((l) => l.sourceId === sourceId);

  const toggleLink = async (source: TaskSource) => {
    if (!projectId) return;
    try {
      if (linkFor(source.id)) {
        await invoke('unlink_task_source', { projectId, sourceId: source.id });
      } else {
        await invoke('link_task_source', { projectId, sourceId: source.id, scopeId: null, branchTemplate: null });
      }
      setLinks(await invoke<TaskSourceLink[]>('list_task_source_links', { projectId }));
    } catch (e) {
      toastError(String(e));
    }
  };

  const updateLink = async (source: TaskSource, patch: Partial<TaskSourceLink>) => {
    const current = linkFor(source.id);
    if (!current) return;
    try {
      await invoke('link_task_source', {
        projectId,
        sourceId: source.id,
        scopeId: patch.scopeId !== undefined ? patch.scopeId || null : current.scopeId,
        branchTemplate: patch.branchTemplate !== undefined ? patch.branchTemplate || null : current.branchTemplate,
      });
      setLinks(await invoke<TaskSourceLink[]>('list_task_source_links', { projectId }));
    } catch (e) {
      toastError(String(e));
    }
  };

  return (
    <div className={styles.screen}>
      <SettingsTabs active="task-settings" title="Tasks" subtitle="where the ☰ Tasks panel gets its rows" />

      <div className={styles.body}>
        <section className={styles.section}>
          <span className={styles.sectionTitle}>Sources</span>
          {sources.length === 0 && <span className={styles.empty}>None yet. A source is your account on a board — add one below.</span>}
          {sources.map((source) => (
            <div key={source.id} className={styles.sourceRow}>
              <span className={styles.sourceName}>{source.label}</span>
              <span className={styles.sourceKind}>{TASK_SOURCE_META[source.kind]?.name ?? source.kind}</span>
              {/* Which account it speaks as. Shown because a token that works
                  and a token that's someone else's look identical until a
                  branch lands in the wrong place. */}
              {typeof source.config.login === 'string' && source.config.login && <span className={styles.sourceLogin}>as {source.config.login}</span>}
              {source.hasCredential && <span className={styles.tokenTag}>token stored</span>}
              <span className={styles.spacer} />
              <button className={styles.removeLink} onClick={() => remove(source)}>
                remove
              </button>
            </div>
          ))}

          {adding === null ? (
            <div className={styles.addRow}>
              {(Object.keys(TASK_SOURCE_META) as TaskSourceKind[]).map((kind) => (
                <Button key={kind} size="sm" variant="outline" onClick={() => setAdding(kind)}>
                  + {TASK_SOURCE_META[kind].name}
                </Button>
              ))}
            </div>
          ) : (
            <div className={styles.form}>
              <span className={styles.formTitle}>{TASK_SOURCE_META[adding].name}</span>
              <span className={styles.hint}>{TASK_SOURCE_META[adding].hint}</span>
              <div className={styles.formRow}>
                <input className={styles.input} placeholder={`name it — e.g. ${TASK_SOURCE_META[adding].name}`} value={label} onChange={(e) => setLabel(e.target.value)} />
                {TASK_SOURCE_META[adding].needsToken && (
                  <input className={styles.input} type="password" placeholder="pk_..." value={token} onChange={(e) => setToken(e.target.value)} />
                )}
              </div>
              <span className={styles.hint}>
                Stored in the macOS keychain, never in the database and never sent to the frontend. It's checked against the tool before it's
                saved — a wrong token otherwise comes back as an empty list, which looks exactly like having no work.
              </span>
              <div className={styles.formActions}>
                <Button size="sm" variant="outline" onClick={() => setAdding(null)}>
                  cancel
                </Button>
                <Button size="sm" disabled={saving} onClick={() => save(adding)}>
                  {saving ? 'checking…' : 'Add source'}
                </Button>
              </div>
            </div>
          )}
        </section>

        <section className={styles.section}>
          <span className={styles.sectionTitle}>Which source feeds which project</span>
          <select className={styles.select} value={projectId} onChange={(e) => setProjectId(e.target.value)}>
            <option value="">project…</option>
            {projects.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>

          {projectId &&
            sources.map((source) => {
              const link = linkFor(source.id);
              return (
                <div key={source.id} className={styles.linkRow}>
                  <label className={styles.linkToggle}>
                    <input type="checkbox" checked={Boolean(link)} onChange={() => toggleLink(source)} />
                    {source.label}
                  </label>
                  {link && (
                    <>
                      <input
                        className={styles.inputSmall}
                        placeholder={source.kind === 'github' ? "owner/repo — blank uses this project's remote" : 'list id — blank means everything assigned to you'}
                        defaultValue={link.scopeId ?? ''}
                        onBlur={(e) => updateLink(source, { scopeId: e.target.value.trim() })}
                      />
                      <input
                        className={styles.inputSmall}
                        placeholder={source.kind === 'github' ? 'issue-{key}-{slug}' : '{key}-{slug}'}
                        defaultValue={link.branchTemplate ?? ''}
                        onBlur={(e) => updateLink(source, { branchTemplate: e.target.value.trim() })}
                      />
                    </>
                  )}
                </div>
              );
            })}
          {projectId && sources.length > 0 && (
            <span className={styles.hint}>
              Branch template placeholders: <code>{'{key}'}</code> the task's key, <code>{'{slug}'}</code> its title, <code>{'{id}'}</code> the
              tool's raw id. Whatever it produces is made safe for <code>git branch</code> before it's used.
            </span>
          )}
        </section>
      </div>
    </div>
  );
}
