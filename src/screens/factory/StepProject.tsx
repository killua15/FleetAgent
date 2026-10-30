import { useEffect, useState } from 'react';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { invoke } from '../../lib/invoke';
import { toastError } from '../../components/ErrorOverlay';
import { useNavigation } from '../../nav/NavigationProvider';
import type { Project } from '../../types/session';
import type { SourceScope, TaskSource } from '../../types/task';
import {
  MODE_HINT,
  MODE_LABEL,
  tildePath,
  type FactoryDelivery,
  type FactoryMode,
  type FactoryProject,
  type FactoryProvider,
  type NewFolder,
  type RepoStatus,
} from '../../types/factory';
import { StepFoot } from './Wizard';
import styles from '../Factory.module.css';

const MODES: FactoryMode[] = ['supervised', 'delegated', 'autonomous'];

function basename(path: string): string {
  return path.replace(/\/+$/, '').split('/').pop() ?? '';
}

function parentOf(path: string): string {
  const trimmed = path.replace(/\/+$/, '');
  return trimmed.slice(0, trimmed.lastIndexOf('/')) || '/';
}

/**
 * Step 1 · Proyecto: where the project lives, its repo, where its tasks come
 * from, how a finished order is delivered and how much the line may decide on
 * its own. With no `project` it's the form that creates one.
 */
export function StepProject({
  project,
  home,
  onCreated,
  onSaved,
  onNext,
}: {
  project: FactoryProject | null;
  home: string | null;
  onCreated: (p: FactoryProject) => void;
  onSaved: (p: FactoryProject) => void;
  onNext: () => void;
}) {
  const { navigate } = useNavigation();
  const draft = project === null;

  const [name, setName] = useState(project?.projectName ?? '');
  const [nameTouched, setNameTouched] = useState(!draft);
  const [folderKind, setFolderKind] = useState<NewFolder['kind']>('existing');
  const [existingPath, setExistingPath] = useState('');
  const [newParent, setNewParent] = useState('');
  const [newName, setNewName] = useState('');

  const [provider, setProvider] = useState<FactoryProvider>(project?.providerKind ?? 'none');
  const [sourceId, setSourceId] = useState<string | null>(project?.providerSourceId ?? null);
  const [scopeId, setScopeId] = useState<string | null>(project?.providerScopeId ?? null);
  const [scopeLabel, setScopeLabel] = useState<string | null>(project?.providerScopeLabel ?? null);
  const [providerTouched, setProviderTouched] = useState(false);
  const [delivery, setDelivery] = useState<FactoryDelivery>(project?.delivery ?? 'local');
  const [deliveryTouched, setDeliveryTouched] = useState(!draft);
  const [mode, setMode] = useState<FactoryMode>(project?.mode ?? 'supervised');
  const [appStartCmd, setAppStartCmd] = useState(project?.appStartCmd ?? '');
  const [appUrl, setAppUrl] = useState(project?.appUrl ?? '');

  const [repo, setRepo] = useState<RepoStatus | null>(null);
  const [checkingRepo, setCheckingRepo] = useState(false);
  const [sources, setSources] = useState<TaskSource[]>([]);
  const [scopes, setScopes] = useState<SourceScope[] | null>(null);
  const [saving, setSaving] = useState(false);
  const [initing, setIniting] = useState(false);

  const isNewFolder = draft && folderKind === 'new';
  const repoPath = project ? project.projectPath : folderKind === 'existing' ? existingPath.trim() : '';
  const newPath = `${newParent.trim().replace(/\/+$/, '')}/${newName.trim()}`;

  useEffect(() => {
    invoke<TaskSource[]>('list_task_sources')
      .then(setSources)
      .catch(() => setSources([]));
    if (draft) {
      // A new project most likely sits next to the last one you opened.
      invoke<Project[]>('list_projects')
        .then((all) => {
          const recent = all.filter((p) => !p.hidden).sort((a, b) => b.createdAt.localeCompare(a.createdAt))[0];
          setNewParent(recent ? tildePath(parentOf(recent.path), home) : '~');
        })
        .catch(() => setNewParent('~'));
    }
  }, [draft, home]);

  // The folder's repo, re-read as the path is typed.
  useEffect(() => {
    if (!repoPath) {
      setRepo(null);
      return;
    }
    setCheckingRepo(true);
    const timer = setTimeout(() => {
      invoke<RepoStatus>('factory_repo_status', { path: repoPath })
        .then(setRepo)
        .catch(() => setRepo(null))
        .finally(() => setCheckingRepo(false));
    }, 300);
    return () => clearTimeout(timer);
  }, [repoPath]);

  // Until you choose otherwise, a repo with a remote delivers by merge
  // request and one without merges locally.
  useEffect(() => {
    if (deliveryTouched) return;
    setDelivery(!isNewFolder && repo?.remoteUrl ? 'mr' : 'local');
  }, [repo, isNewFolder, deliveryTouched]);

  // A new project's name follows its folder until you type one.
  useEffect(() => {
    if (nameTouched) return;
    setName(folderKind === 'new' ? newName.trim() : basename(existingPath.trim()));
  }, [folderKind, newName, existingPath, nameTouched]);

  const source = sources.find((s) => s.id === sourceId) ?? null;

  useEffect(() => {
    setScopes(null);
    if (!source || source.kind !== 'clickup') return;
    invoke<SourceScope[]>('list_source_scopes', { sourceId: source.id })
      .then(setScopes)
      .catch(() => setScopes([]));
  }, [source?.id, source?.kind]);

  const chooseProvider = (value: string) => {
    setProviderTouched(true);
    if (value === 'none') {
      setProvider('none');
      setSourceId(null);
      setScopeId(null);
      setScopeLabel(null);
      return;
    }
    const picked = sources.find((s) => s.id === value);
    if (!picked) return;
    setProvider(picked.kind);
    setSourceId(picked.id);
    // GitHub's scope is a repo, and the project's own remote is nearly always it.
    const ownRepo = picked.kind === 'github' && repo?.forge === 'github' ? repo.forgePath : null;
    setScopeId(ownRepo);
    setScopeLabel(ownRepo);
  };

  const chooseScope = (id: string) => {
    setProviderTouched(true);
    setScopeId(id || null);
    setScopeLabel(scopes?.find((s) => s.id === id)?.path ?? null);
  };

  const pickFolder = async (apply: (path: string) => void) => {
    const selected = await openDialog({ directory: true, multiple: false, title: 'Elegir carpeta' });
    if (typeof selected === 'string') apply(tildePath(selected, home));
  };

  const providerReady = provider === 'none' || (!!sourceId && !!scopeId);
  const folderReady = draft ? (folderKind === 'existing' ? !!repo?.exists : !!newParent.trim() && !!newName.trim()) : true;

  const form = (base: FactoryProject): FactoryProject => {
    const keepServerProvider = draft && !providerTouched;
    return {
      ...base,
      providerKind: keepServerProvider ? base.providerKind : provider,
      providerSourceId: keepServerProvider ? base.providerSourceId : sourceId,
      providerScopeId: keepServerProvider ? base.providerScopeId : scopeId,
      providerScopeLabel: keepServerProvider ? base.providerScopeLabel : scopeLabel,
      delivery,
      mode,
      appStartCmd: appStartCmd.trim() || null,
      appUrl: appUrl.trim() || null,
    };
  };

  const next = project && form(project);
  const dirty =
    !project ||
    !next ||
    name.trim() !== project.projectName ||
    next.providerKind !== project.providerKind ||
    next.providerSourceId !== project.providerSourceId ||
    next.providerScopeId !== project.providerScopeId ||
    next.delivery !== project.delivery ||
    next.mode !== project.mode ||
    next.appStartCmd !== project.appStartCmd ||
    next.appUrl !== project.appUrl;

  const rename = async (p: FactoryProject): Promise<FactoryProject> => {
    const wanted = name.trim();
    if (!wanted || wanted === p.projectName) return p;
    await invoke('update_project', { id: p.projectId, name: wanted });
    return { ...p, projectName: wanted };
  };

  const save = async () => {
    setSaving(true);
    try {
      if (draft) {
        const folder: NewFolder =
          folderKind === 'existing' ? { kind: 'existing', path: existingPath.trim() } : { kind: 'new', parent: newParent.trim(), name: newName.trim() };
        const created = await rename(await invoke<FactoryProject>('factory_create_project', { folder }));
        onCreated(await invoke<FactoryProject>('factory_update_project', { project: form(created) }));
      } else {
        const renamed = await rename(project);
        onSaved(await invoke<FactoryProject>('factory_update_project', { project: form(renamed) }));
      }
    } catch (e) {
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const initRepo = async () => {
    if (!project) return;
    setIniting(true);
    try {
      setRepo(await invoke<RepoStatus>('factory_init_repo', { projectId: project.projectId }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setIniting(false);
    }
  };

  const noRemote = isNewFolder || !repo?.remoteUrl;
  const base = repo?.baseBranch ?? 'dev';
  const usable = sources.filter((s) => s.kind === 'clickup' || s.kind === 'github');

  return (
    <>
      <div className={styles.stepBody}>
        <span className={styles.lead}>El proyecto: dónde vive, cómo entrega y con qué cuentas trabaja</span>
        <div className={styles.card}>
          <div className={styles.form}>
            <label htmlFor="factory-name">Nombre</label>
            <input
              id="factory-name"
              className={`${styles.input} ${styles.narrow}`}
              value={name}
              placeholder="nombre del proyecto"
              onChange={(e) => {
                setNameTouched(true);
                setName(e.target.value);
              }}
            />

            <label>Carpeta</label>
            {draft ? (
              <div className={styles.col}>
                <div className={styles.seg}>
                  <button aria-pressed={folderKind === 'existing'} onClick={() => setFolderKind('existing')}>
                    Carpeta existente
                  </button>
                  <button aria-pressed={folderKind === 'new'} onClick={() => setFolderKind('new')}>
                    Crear carpeta nueva
                  </button>
                </div>
                {folderKind === 'existing' ? (
                  <div className={styles.row}>
                    <input
                      className={`${styles.input} ${styles.mono}`}
                      style={{ flex: 1 }}
                      value={existingPath}
                      placeholder="~/Proyects/mi-proyecto"
                      onChange={(e) => setExistingPath(e.target.value)}
                    />
                    <button className={`${styles.btn} ${styles.sm}`} onClick={() => pickFolder(setExistingPath)}>
                      Elegir…
                    </button>
                  </div>
                ) : (
                  <div className={styles.row}>
                    <input className={`${styles.input} ${styles.mono}`} style={{ maxWidth: 240 }} value={newParent} onChange={(e) => setNewParent(e.target.value)} />
                    <button className={`${styles.btn} ${styles.sm}`} onClick={() => pickFolder(setNewParent)}>
                      Elegir…
                    </button>
                    <span className={styles.slash}>/</span>
                    <input
                      className={`${styles.input} ${styles.mono}`}
                      style={{ maxWidth: 200 }}
                      value={newName}
                      placeholder="nombre-de-carpeta"
                      onChange={(e) => setNewName(e.target.value)}
                    />
                    {newName.trim() && <span className={styles.hint}>se creará {newPath}</span>}
                  </div>
                )}
              </div>
            ) : (
              <span className={`${styles.mono} ${styles.hint}`}>{tildePath(project.projectPath, home)}</span>
            )}

            <label>Repositorio</label>
            <div className={styles.row}>
              {isNewFolder ? (
                <>
                  <span className={styles.chipTeal}>nuevo</span>
                  <span className={styles.hint}>
                    Se crea la carpeta y un repo git con <span className={styles.mono}>main</span> y <span className={styles.mono}>dev</span>, sin remoto.
                    Puedes publicarlo en GitLab o GitHub más adelante.
                  </span>
                </>
              ) : !repoPath ? (
                <span className={styles.hint}>Elige una carpeta.</span>
              ) : checkingRepo && !repo ? (
                <span className={styles.hint}>comprobando…</span>
              ) : !repo?.exists ? (
                <span className={styles.chipRed}>no existe</span>
              ) : !repo.isRepo ? (
                <>
                  <span className={styles.chipAmber}>sin git</span>
                  {project ? (
                    <button className={`${styles.btn} ${styles.sm}`} disabled={initing} onClick={initRepo}>
                      {initing ? 'inicializando…' : 'Inicializar git (main + dev)'}
                    </button>
                  ) : (
                    <span className={styles.hint}>No es un repo git: al crear el proyecto podrás inicializarlo con main y dev.</span>
                  )}
                </>
              ) : repo.remoteUrl ? (
                <>
                  <span className={styles.chipGrn}>✓ detectado</span>
                  <span className={styles.mono} style={{ fontSize: 12 }}>
                    {repo.remoteUrl} · rama base {base}
                  </span>
                </>
              ) : (
                <>
                  <span className={styles.chipTeal}>local</span>
                  <span className={styles.mono} style={{ fontSize: 12 }}>
                    sin remoto · rama base {base}
                  </span>
                </>
              )}
              {repo?.isRepo && !repo.hasCommits && <span className={`${styles.hint} ${styles.warn}`}>sin commits todavía</span>}
              {repo?.isRepo && !repo.hasDev && <span className={`${styles.hint} ${styles.warn}`}>no hay rama dev: las órdenes saldrán de {base}</span>}
            </div>

            <label>Proveedor de tareas</label>
            <div className={styles.col}>
              <div className={styles.row}>
                <select className={styles.select} style={{ maxWidth: 300 }} value={sourceId ?? 'none'} onChange={(e) => chooseProvider(e.target.value)}>
                  {usable.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.kind === 'clickup' ? `ClickUp · cuenta «${s.label}»` : `GitHub · ${s.label}`}
                    </option>
                  ))}
                  <option value="none">Ninguno por ahora · tareas en local</option>
                </select>
                {provider === 'clickup' && (
                  <select className={styles.select} style={{ maxWidth: 320 }} value={scopeId ?? ''} onChange={(e) => chooseScope(e.target.value)} disabled={scopes === null}>
                    <option value="">{scopes === null ? 'cargando listas…' : scopes.length === 0 ? 'no se pudieron leer las listas' : 'elige la lista del proyecto'}</option>
                    {/* A list saved earlier that the account no longer shows stays selectable. */}
                    {scopeId && scopes && !scopes.some((s) => s.id === scopeId) && <option value={scopeId}>{scopeLabel ?? scopeId}</option>}
                    {scopes?.map((s) => (
                      <option key={s.id} value={s.id}>
                        {s.path}
                      </option>
                    ))}
                  </select>
                )}
                {provider === 'github' && (
                  <input
                    className={`${styles.input} ${styles.mono}`}
                    style={{ maxWidth: 260 }}
                    value={scopeId ?? ''}
                    placeholder="owner/repo"
                    onChange={(e) => {
                      setProviderTouched(true);
                      setScopeId(e.target.value.trim() || null);
                      setScopeLabel(e.target.value.trim() || null);
                    }}
                  />
                )}
              </div>
              {provider === 'none' ? (
                <span className={styles.hint}>Las tareas viven en Hangar. Cuando conectes ClickUp o GitHub, se exportan las que nadie empezó.</span>
              ) : (
                <span className={styles.hint}>
                  Solo entran las tareas de {provider === 'clickup' ? 'esa lista' : 'las issues de ese repo'}: nada de otros proyectos.
                </span>
              )}
              {usable.length === 0 && (
                <div className={styles.row}>
                  <span className={styles.hint}>No hay cuentas de ClickUp ni GitHub en Hangar.</span>
                  <button className={`${styles.btnGhost} ${styles.sm}`} onClick={() => navigate('task-settings')}>
                    Conectar una en Settings → Tasks
                  </button>
                </div>
              )}
            </div>

            <label>Al terminar una orden</label>
            <div className={styles.row}>
              <div className={styles.seg}>
                <button
                  aria-pressed={delivery === 'mr'}
                  onClick={() => {
                    setDeliveryTouched(true);
                    setDelivery('mr');
                  }}
                >
                  Abrir MR → {base}
                </button>
                <button
                  aria-pressed={delivery === 'local'}
                  onClick={() => {
                    setDeliveryTouched(true);
                    setDelivery('local');
                  }}
                >
                  Merge local
                </button>
              </div>
              {delivery === 'mr' && noRemote && <span className={`${styles.hint} ${styles.warn}`}>sin remoto no hay dónde abrir la MR</span>}
              {delivery === 'local' && (
                <span className={styles.hint}>
                  {noRemote ? `sin remoto: se fusiona en ${base} local hasta que lo publiques` : `se fusiona en ${base} local; nunca hace push`}
                </span>
              )}
            </div>

            <label>Modo</label>
            <div className={styles.row}>
              <div className={styles.seg}>
                {MODES.map((m) => (
                  <button key={m} aria-pressed={mode === m} onClick={() => setMode(m)}>
                    {MODE_LABEL[m]}
                  </button>
                ))}
              </div>
              <span className={styles.hint}>{MODE_HINT[mode]}</span>
            </div>

            <span className={styles.formRule} />

            <label>App del proyecto</label>
            <div className={styles.col}>
              <div className={styles.row}>
                <input
                  className={`${styles.input} ${styles.mono}`}
                  style={{ maxWidth: 260 }}
                  value={appStartCmd}
                  placeholder="cómo se arranca · npm run dev"
                  onChange={(e) => setAppStartCmd(e.target.value)}
                />
                <input
                  className={`${styles.input} ${styles.mono}`}
                  style={{ maxWidth: 260 }}
                  value={appUrl}
                  placeholder="dónde se ve · http://localhost:5173"
                  onChange={(e) => setAppUrl(e.target.value)}
                />
              </div>
              <span className={styles.hint}>
                Opcional. Los revisores de UI y UX la abren en Chrome; si no arranca, lo dicen y la línea sigue.
              </span>
            </div>
          </div>
        </div>
      </div>

      <StepFoot
        step={1}
        hint={draft ? 'Elige la carpeta y crea el proyecto.' : dirty ? 'Guarda los cambios para seguir.' : 'Revisa el proyecto y sigue.'}
        onNext={onNext}
        nextDisabled={draft || dirty}
        extra={
          <>
            {!draft && !dirty && <span className={`${styles.hint} ${styles.ok}`}>✓ guardado</span>}
            {(draft || dirty) && (
              <button className={styles.btnPrimary} disabled={saving || !folderReady || !providerReady || !name.trim()} onClick={save}>
                {saving ? (draft ? 'creando…' : 'guardando…') : draft ? (isNewFolder ? 'Crear carpeta y proyecto' : 'Crear proyecto') : 'Guardar'}
              </button>
            )}
          </>
        }
      />
    </>
  );
}
