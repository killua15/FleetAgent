import { useCallback, useEffect, useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { openUrl } from '@tauri-apps/plugin-opener';
import { invoke } from '../lib/invoke';
import { SettingsTabs } from '../components/SettingsTabs';
import { Spinner } from '../components/Spinner';
import { toastError, toastSuccess } from '../components/ErrorOverlay';
import { forgeHostLabel } from '../lib/forgeHosts';
import type { CapabilityStatus, ToolStatus } from '../types/requirements';
import { DECLARED_KIND_LABEL, type DeclaredKind, type ForgeInstanceStatus, type InstanceRepo } from '../types/forgeStatus';
import styles from './ForgeSettings.module.css';

/**
 * Where forges are set up: the CLIs Hangar drives, the instances your projects
 * live on, and the repositories on them.
 *
 * Its own screen rather than a section of General for one reason: General
 * answers "what does this machine need", which is true of the machine and
 * nothing else. Everything here is about *a specific server* — which one, who
 * you are on it, what it holds. Those are different questions, and the second
 * one grows.
 */
export function ForgeSettings() {
  const [instances, setInstances] = useState<ForgeInstanceStatus[]>([]);
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [checking, setChecking] = useState(false);
  const [busyHost, setBusyHost] = useState<string | null>(null);

  const loadTools = useCallback(async () => {
    // The CLI rows are a view over what the capabilities already declare —
    // deriving them here rather than keeping a second list is the same rule
    // `requirements.rs` states about itself.
    const caps = await invoke<CapabilityStatus[]>('list_requirements').catch(() => [] as CapabilityStatus[]);
    const wanted = caps.filter((c) => c.id === 'pull-requests' || c.id === 'merge-requests');
    const byName = new Map<string, ToolStatus>();
    for (const tool of wanted.flatMap((c) => c.tools)) {
      if (tool.name !== 'git') byName.set(tool.name, tool);
    }
    setTools([...byName.values()]);
  }, []);

  const load = useCallback(async () => {
    try {
      const [found] = await Promise.all([invoke<ForgeInstanceStatus[]>('list_forge_instances'), loadTools()]);
      setInstances(found);
    } catch (e) {
      toastError(String(e));
    } finally {
      setLoading(false);
    }
  }, [loadTools]);

  useEffect(() => {
    load();
  }, [load]);

  const recheck = async () => {
    setChecking(true);
    try {
      setInstances(await invoke<ForgeInstanceStatus[]>('recheck_forge_instances'));
      await loadTools();
    } catch (e) {
      toastError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const declare = async (host: string, kind: DeclaredKind, scheme: string, label: string) => {
    setBusyHost(host);
    try {
      setInstances(await invoke<ForgeInstanceStatus[]>('save_forge_instance', { host, kind, scheme, label }));
      toastSuccess(`${host} declarado como ${DECLARED_KIND_LABEL[kind]}`);
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusyHost(null);
    }
  };

  const forget = async (host: string) => {
    setBusyHost(host);
    try {
      setInstances(await invoke<ForgeInstanceStatus[]>('delete_forge_instance', { host }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusyHost(null);
    }
  };

  const authenticate = async (host: string, token: string) => {
    setBusyHost(host);
    try {
      setInstances(await invoke<ForgeInstanceStatus[]>('authenticate_forge_instance', { host, token }));
      toastSuccess(`Sesión iniciada en ${host}`);
    } catch (e) {
      toastError(String(e));
    } finally {
      setBusyHost(null);
    }
  };

  return (
    <div className={styles.screen}>
      <SettingsTabs active="forge-settings" title="Control de versiones" />

      <div className={styles.body}>
        <section className={styles.section}>
          <div className={styles.sectionHead}>
            <span className={styles.sectionLabel}>CLIS</span>
            <span className={styles.spacer} />
            <button className={styles.btn} onClick={recheck} disabled={checking}>
              {checking ? 'comprobando…' : 'volver a comprobar'}
            </button>
          </div>
          <span className={styles.hint}>
            Hangar no habla con las APIs directamente: conduce <code>gh</code> y <code>glab</code>. Si falta uno, lo que falla es su forge,
            no la app entera.
          </span>
          {tools.map((tool) => (
            <div key={tool.name} className={styles.cliRow}>
              <span className={styles.cliName}>{tool.name}</span>
              <span className={tool.installed ? styles.ok : styles.bad}>{tool.installed ? '✓ instalado' : '⚠ falta'}</span>
              {tool.installed ? (
                <span className={styles.cliPath}>{tool.path}</span>
              ) : (
                tool.installHint && <code className={styles.command}>{tool.installHint}</code>
              )}
            </div>
          ))}
        </section>

        <section className={styles.section}>
          <span className={styles.sectionLabel}>INSTANCIAS</span>
          <span className={styles.hint}>
            Cada host que sale del <code>origin</code> de tus proyectos. Lo que declares aquí gana a cualquier heurística — es la única
            forma de colocar un GitHub Enterprise en dominio propio, y <strong>solo git</strong> es lo que hace que Hangar deje de sondear
            un host que no tiene API.
          </span>
          {loading && <Spinner label="mirando los remotes…" />}
          {!loading && instances.length === 0 && <span className={styles.empty}>Ningún proyecto tiene todavía un remote.</span>}
          {instances.map((instance) => (
            <InstanceRow
              key={instance.host}
              instance={instance}
              busy={busyHost === instance.host}
              onDeclare={declare}
              onForget={forget}
              onAuthenticate={authenticate}
            />
          ))}
          <DeclareForm busy={busyHost !== null} onDeclare={declare} />
        </section>

        <RepositoriesSection instances={instances} onCloned={load} />
      </div>
    </div>
  );
}

function InstanceRow({
  instance,
  busy,
  onDeclare,
  onForget,
  onAuthenticate,
}: {
  instance: ForgeInstanceStatus;
  busy: boolean;
  onDeclare: (host: string, kind: DeclaredKind, scheme: string, label: string) => void;
  onForget: (host: string) => void;
  onAuthenticate: (host: string, token: string) => void;
}) {
  const [token, setToken] = useState('');
  const [showToken, setShowToken] = useState(false);

  // A git-only instance is working exactly as declared, so it isn't a problem
  // row even though it has no session.
  const broken = !instance.authenticated && !instance.gitOnly;

  return (
    <div className={broken ? styles.rowBad : styles.row}>
      <div className={styles.rowHead}>
        <span className={styles.host}>{instance.label || (instance.forge ? forgeHostLabel({ host: instance.host, forge: instance.forge }) : instance.host)}</span>
        <span className={styles.meta}>{instance.host}</span>
        {instance.declared ? <span className={styles.tagDeclared}>declarado</span> : <span className={styles.tag}>deducido</span>}
        {instance.gitOnly && <span className={styles.tag}>solo git</span>}
        {!instance.gitOnly && <span className={instance.authenticated ? styles.ok : styles.bad}>{instance.authenticated ? '✓ con sesión' : '⚠ sin sesión'}</span>}
        {instance.viewerLogin && <span className={styles.meta}>@{instance.viewerLogin}</span>}
        <span className={styles.spacer} />
        {instance.declared && (
          <button className={`${styles.btn} ${styles.btnDanger}`} disabled={busy} onClick={() => onForget(instance.host)}>
            Olvidar
          </button>
        )}
      </div>

      {instance.projects.length > 0 && <span className={styles.meta}>{instance.projects.map((p) => p.name).join(' · ')}</span>}
      {instance.reason && <span className={styles.reason}>{instance.reason}</span>}

      {/* Declaring is offered whenever Hangar guessed — including when the guess
          happens to be right, since a wrong guess is invisible until something
          fails much later. */}
      {!instance.declared && (
        <div className={styles.rowHead}>
          <span className={styles.meta}>declarar como</span>
          {(['github', 'gitlab', 'git_only'] as DeclaredKind[]).map((kind) => (
            <button key={kind} className={styles.btn} disabled={busy} onClick={() => onDeclare(instance.host, kind, 'https', '')}>
              {DECLARED_KIND_LABEL[kind]}
            </button>
          ))}
        </div>
      )}

      {broken && instance.forge && (
        <div className={styles.rowHead}>
          <input
            className={styles.input}
            type={showToken ? 'text' : 'password'}
            placeholder="token de acceso personal"
            value={token}
            onChange={(e) => setToken(e.target.value)}
            spellCheck={false}
          />
          <button className={styles.btn} onClick={() => setShowToken((v) => !v)}>
            {showToken ? 'ocultar' : 'ver'}
          </button>
          <button className={styles.btnPrimary} disabled={busy || !token.trim()} onClick={() => onAuthenticate(instance.host, token)}>
            {busy ? 'entrando…' : 'Autenticar'}
          </button>
        </div>
      )}
      {/* The token goes to the CLI over stdin and is never stored by Hangar —
          `gh`/`glab` keep it in the OS keyring. Worth saying where the user is
          being asked to paste one. */}
      {broken && instance.forge && (
        <span className={styles.meta}>El token va por stdin al CLI y lo guarda su llavero, no Hangar.</span>
      )}
      {broken && instance.loginCommand && <code className={styles.command}>{instance.loginCommand}</code>}
    </div>
  );
}

/** For an instance no project points at yet — which is the normal case before
 * cloning anything from it. */
function DeclareForm({ busy, onDeclare }: { busy: boolean; onDeclare: (host: string, kind: DeclaredKind, scheme: string, label: string) => void }) {
  const [host, setHost] = useState('');
  const [kind, setKind] = useState<DeclaredKind>('gitlab');
  const [scheme, setScheme] = useState('https');
  const [label, setLabel] = useState('');

  const submit = () => {
    if (!host.trim()) return;
    onDeclare(host.trim(), kind, scheme, label.trim());
    setHost('');
    setLabel('');
  };

  return (
    <div className={styles.form}>
      <input
        className={styles.input}
        placeholder="host (gitlab.acme.com, 10.0.0.5:3000)"
        value={host}
        onChange={(e) => setHost(e.target.value)}
        onKeyDown={(e) => e.key === 'Enter' && submit()}
        spellCheck={false}
      />
      <select className={styles.select} value={kind} onChange={(e) => setKind(e.target.value as DeclaredKind)}>
        {(['github', 'gitlab', 'git_only'] as DeclaredKind[]).map((k) => (
          <option key={k} value={k}>
            {DECLARED_KIND_LABEL[k]}
          </option>
        ))}
      </select>
      <select className={styles.select} value={scheme} onChange={(e) => setScheme(e.target.value)}>
        <option value="https">https</option>
        <option value="http">http</option>
      </select>
      <input className={styles.input} placeholder="etiqueta (opcional)" value={label} onChange={(e) => setLabel(e.target.value)} />
      <button className={styles.btnPrimary} disabled={busy || !host.trim()} onClick={submit}>
        Añadir instancia
      </button>
    </div>
  );
}

/** Browse an instance and clone from it — the half of "ver los repositorios"
 * that filtering never covered. */
function RepositoriesSection({ instances, onCloned }: { instances: ForgeInstanceStatus[]; onCloned: () => void }) {
  // Only instances that can actually answer: a git-only host has no API, and an
  // unauthenticated one would just fail.
  const browsable = instances.filter((i) => i.authenticated && !i.gitOnly);
  const [host, setHost] = useState<string>('');
  const [repos, setRepos] = useState<InstanceRepo[]>([]);
  const [listing, setListing] = useState(false);
  const [cloning, setCloning] = useState<string | null>(null);

  const active = browsable.some((i) => i.host === host) ? host : (browsable[0]?.host ?? '');

  const list = async () => {
    if (!active) return;
    setListing(true);
    try {
      setRepos(await invoke<InstanceRepo[]>('list_instance_repos', { host: active }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setListing(false);
    }
  };

  const clone = async (repo: InstanceRepo) => {
    // The parent folder is the user's to choose — Hangar has no default worth
    // guessing, and cloning into a surprise location is hard to undo.
    const parent = await open({ directory: true, multiple: false, title: `¿Dónde clonar ${repo.name}?` });
    if (typeof parent !== 'string') return;
    setCloning(repo.path);
    try {
      await invoke('clone_instance_repo', { cloneUrl: repo.cloneUrl, parentDir: parent, path: repo.path });
      toastSuccess(`${repo.name} clonado y añadido como proyecto`);
      onCloned();
    } catch (e) {
      toastError(String(e));
    } finally {
      setCloning(null);
    }
  };

  return (
    <section className={styles.section}>
      <span className={styles.sectionLabel}>REPOSITORIOS</span>
      <span className={styles.hint}>
        Los repos que ve tu sesión en cada instancia. Clonar uno lo registra como proyecto, igual que si lo hubieras añadido con el
        selector de carpetas.
      </span>

      {browsable.length === 0 ? (
        <span className={styles.empty}>Ninguna instancia con sesión todavía — autentica una arriba.</span>
      ) : (
        <>
          <div className={styles.sectionHead}>
            <select className={styles.select} value={active} onChange={(e) => setHost(e.target.value)}>
              {browsable.map((i) => (
                <option key={i.host} value={i.host}>
                  {i.label || i.host}
                </option>
              ))}
            </select>
            <button className={styles.btn} disabled={listing} onClick={list}>
              {listing ? <Spinner label="listando…" /> : 'Listar repositorios'}
            </button>
          </div>

          <div className={styles.repoList}>
            {repos.map((repo) => (
              <div key={repo.path} className={styles.repoRow}>
                <span className={styles.repoPath}>{repo.path}</span>
                {repo.description && <span className={styles.repoDesc}>{repo.description}</span>}
                <span className={styles.spacer} />
                {repo.webUrl && (
                  <button className={styles.btn} onClick={() => openUrl(repo.webUrl)}>
                    ↗
                  </button>
                )}
                <button className={styles.btn} disabled={cloning !== null} onClick={() => clone(repo)}>
                  {cloning === repo.path ? 'clonando…' : 'Clonar'}
                </button>
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  );
}
