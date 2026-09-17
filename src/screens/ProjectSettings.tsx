import { useCallback, useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { SettingsTabs } from '../components/SettingsTabs';
import { toastError } from '../components/ErrorOverlay';
import { Spinner } from '../components/Spinner';
import { AccountPickerButton } from '../components/AccountPickerButton';
import { Button } from '../components/ui/button';
import { LANGUAGES } from '../lib/languages';
import type { Project } from '../types/session';
import type { AgentPreferences, Skill } from '../types/settings';
import type { GraphState, GraphifyStatus } from '../types/graph';
import { ARCHIFY_SKILL, type Diagram } from '../types/diagram';
import { advance, finish, IDLE, STAGE_LABEL, STAGES, type DiagramProgress } from '../lib/diagramProgress';
import { listen } from '@tauri-apps/api/event';
import styles from './ProjectSettings.module.css';

/**
 * Lo que cada proyecto le dice a sus agentes, cuando no le vale lo de General.
 *
 * Hereda por defecto, y esa es la diferencia importante con la pantalla global:
 * aquí un campo vacío no significa "sin preferencia", significa "lo que diga
 * General". Un proyecto nuevo no necesita configurarse para funcionar bien, que
 * es lo que haría inútil tener un valor global.
 *
 * Misma forma que `RotationSettings`: proyectos a la izquierda, ajustes del
 * seleccionado a la derecha. Son la misma pregunta —qué le pasa a *este* repo—
 * y darles dos formas distintas obligaría a aprenderlas dos veces.
 */
const INHERIT = '';

export function ProjectSettings() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [global, setGlobal] = useState<AgentPreferences>({ conversationLanguage: null, commentLanguage: null });
  const [prefs, setPrefs] = useState<AgentPreferences>({ conversationLanguage: null, commentLanguage: null });
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [enabledSkills, setEnabledSkills] = useState<string[]>([]);
  const [alwaysOn, setAlwaysOn] = useState<string[]>([]);
  const [graphify, setGraphify] = useState<GraphifyStatus | null>(null);

  useEffect(() => {
    Promise.all([
      invoke<Project[]>('list_projects').then((list) => {
        setProjects(list);
        setSelectedId((prev) => (prev && list.some((p) => p.id === prev) ? prev : (list[0]?.id ?? null)));
      }),
      invoke<AgentPreferences>('get_global_agent_preferences').then(setGlobal),
      invoke<Skill[]>('list_skills').then(setSkills),
      invoke<string[]>('list_always_on_skills').then(setAlwaysOn),
      invoke<GraphifyStatus>('graphify_status').then(setGraphify),
    ]).catch((e) => toastError(String(e)));
  }, []);

  useEffect(() => {
    if (!selectedId) return;
    setLoading(true);
    Promise.all([
      invoke<AgentPreferences>('get_project_agent_preferences', { projectId: selectedId }).then(setPrefs),
      invoke<string[]>('list_project_skills', { projectId: selectedId }).then(setEnabledSkills),
    ])
      .catch((e) => toastError(String(e)))
      .finally(() => setLoading(false));
  }, [selectedId]);

  const toggleSkill = async (skillName: string, enabled: boolean) => {
    if (!selectedId) return;
    const previous = enabledSkills;
    // Optimista: marcar una casilla tiene que sentirse inmediato, y el
    // servidor devuelve la lista real para corregir si algo falló.
    setEnabledSkills((prev) => (enabled ? [...prev, skillName] : prev.filter((n) => n !== skillName)));
    try {
      setEnabledSkills(await invoke<string[]>('set_project_skill', { projectId: selectedId, skillName, enabled }));
    } catch (e) {
      setEnabledSkills(previous);
      toastError(String(e));
    }
  };

  const save = async (next: AgentPreferences) => {
    if (!selectedId) return;
    const previous = prefs;
    setPrefs(next);
    setSaving(true);
    try {
      setPrefs(await invoke<AgentPreferences>('set_project_agent_preferences', { projectId: selectedId, input: next }));
    } catch (e) {
      setPrefs(previous);
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const selected = projects.find((p) => p.id === selectedId);
  const overrides = [prefs.conversationLanguage, prefs.commentLanguage].filter(Boolean).length;

  return (
    <div className={styles.screen}>
      <SettingsTabs
        active="project-settings"
        title="Projects"
        subtitle="lo que cada proyecto le dice a sus agentes. Sin tocar nada, hereda lo de General."
      />
      <div className={styles.body}>
        <div className={styles.projectList}>
          <span className={styles.sectionLabel}>PROJECTS</span>
          {projects.map((p) => (
            <button
              key={p.id}
              className={p.id === selectedId ? styles.projectRowActive : styles.projectRow}
              onClick={() => setSelectedId(p.id)}
            >
              <span className={styles.projectIcon}>{p.name[0]?.toUpperCase()}</span>
              <span className={styles.projectName}>{p.name}</span>
            </button>
          ))}
          {projects.length === 0 && <span className={styles.empty}>Ningún proyecto todavía.</span>}
          <div className={styles.spacer} />
          <p className={styles.footnote}>
            Se aplica a las terminales y turnos que empiecen a partir de ahora — las preferencias se le pasan al CLI al arrancarlo,
            así que las que ya están abiertas siguen como estaban.
          </p>
        </div>

        <div className={styles.panel}>
          {selected && (
            <>
              <div className={styles.panelHeader}>
                <span className={styles.panelTitle}>{selected.name}</span>
                <span className={styles.dimText}>
                  {loading ? <Spinner label="cargando…" /> : overrides === 0 ? 'hereda todo de General' : `${overrides} ajuste${overrides === 1 ? '' : 's'} propio${overrides === 1 ? '' : 's'}`}
                </span>
              </div>

              <LanguageField
                label="IDIOMA DE LA CONVERSACIÓN"
                hint="En el que te responde: explicaciones, resúmenes y preguntas."
                value={prefs.conversationLanguage}
                inherited={global.conversationLanguage}
                disabled={saving}
                onChange={(v) => save({ ...prefs, conversationLanguage: v })}
              />

              <LanguageField
                label="IDIOMA DEL CÓDIGO"
                hint="Comentarios, nombres de identificadores, mensajes de commit y títulos de PR. Independiente del anterior."
                value={prefs.commentLanguage}
                inherited={global.commentLanguage}
                disabled={saving}
                onChange={(v) => save({ ...prefs, commentLanguage: v })}
              />

              <div className={styles.field}>
                <span className={styles.fieldLabel}>SKILLS EN ESTE PROYECTO</span>
                <span className={styles.fieldHint}>
                  Las que marques se activan aquí y se invocan con <code>/nombre</code>. Si el repo trae las suyas en{' '}
                  <code>.claude/skills/</code>, se cargan solas y estas se <strong>suman</strong>, no las reemplazan.
                </span>
                {skills.length === 0 ? (
                  <span className={styles.empty}>
                    Todavía no tienes ninguna skill. Se crean en <strong>Settings → skills</strong>.
                  </span>
                ) : (
                  <div className={styles.skillList}>
                    {skills.map((skill) => {
                      // Una skill siempre activa ya está en todos los proyectos
                      // por otro camino. Se muestra marcada y bloqueada porque
                      // desmarcarla aquí no haría nada: una casilla que no
                      // obedece se lee como un fallo.
                      const always = alwaysOn.includes(skill.name);
                      return (
                        <label key={skill.name} className={styles.skillRow}>
                          <input
                            type="checkbox"
                            checked={always || enabledSkills.includes(skill.name)}
                            disabled={saving || always}
                            onChange={(e) => toggleSkill(skill.name, e.target.checked)}
                          />
                          <span className={styles.skillInfo}>
                            <span className={styles.skillName}>
                              /{skill.name}
                              {always && <span className={styles.skillBadge}>siempre activa</span>}
                              {skill.hasExtraFiles && <span className={styles.skillBadge}>+ ficheros</span>}
                            </span>
                            <span className={styles.skillDescription}>
                              {always
                                ? 'Activa en todos los proyectos desde Settings → skills.'
                                : skill.description || 'sin descripción — no se activará sola'}
                            </span>
                          </span>
                        </label>
                      );
                    })}
                  </div>
                )}
              </div>

              <CodeGraphField projectId={selected.id} graphify={graphify} />

              <DiagramsField
                projectId={selected.id}
                installed={skills.some((sk) => sk.name === ARCHIFY_SKILL)}
                enabled={enabledSkills.includes(ARCHIFY_SKILL)}
                onEnable={() => toggleSkill(ARCHIFY_SKILL, true)}
              />

              <p className={styles.footnote}>
                Idiomas y skills llegan hoy a Claude, Kimi y Grok. Codex recibe el idioma por su propia configuración, pero todavía no
                las skills — busca en otra ruta y no he podido verificarlo contra una ejecución real. Sus terminales funcionan
                igual que hasta ahora.
              </p>
            </>
          )}
          {!selected && <span className={styles.empty}>Elige un proyecto para configurarlo.</span>}
        </div>
      </div>
    </div>
  );
}

/**
 * El grafo de código de este proyecto (`GRAPHIFY_TRD.md`, Fase 1).
 *
 * Tres cosas que esta sección tiene que decir sin que haya que preguntarlas:
 * si la herramienta está o no, cuándo se construyó el grafo, y si el repo ha
 * avanzado desde entonces. La tercera es la que importa — §5.5: un grafo viejo
 * responde con la forma de la verdad y el contenido equivocado.
 *
 * Sin `graphify` instalado esto no es un error ni un hueco: dice qué instalar y
 * se queda quieto.
 */
/**
 * Los diagramas que un agente ya entregó para este proyecto.
 *
 * Se abren en el navegador del sistema, no aquí, por la misma razón que el HTML
 * del grafo: una página así se renderiza en blanco bajo el CSP de la app. Y hay
 * una segunda razón propia — el HTML lo escribió lo que un agente compuso, y el
 * navegador es mejor sitio para eso que el webview de la aplicación.
 *
 * La lista es del directorio que Hangar nombra, no del repo. Un diagrama que el
 * agente dejara en otro sitio no aparece, y el texto de vacío lo dice en vez de
 * dejar pensando que no hay ninguno.
 */
function DiagramsField({
  projectId,
  installed,
  enabled,
  onEnable,
}: {
  projectId: string;
  installed: boolean;
  enabled: boolean;
  onEnable: () => void;
}) {
  const [diagrams, setDiagrams] = useState<Diagram[] | null>(null);
  const [asking, setAsking] = useState(false);
  /** The turn currently drawing, and how far it has got. `null` when nothing
   * is running — this lives only while the screen is open, so leaving and
   * coming back loses the live view but not the work: it is a session, and it
   * carries on. */
  const [running, setRunning] = useState<{ sessionId: string; progress: DiagramProgress } | null>(null);
  /** El `invoke` vuelve en milisegundos y el turno dura minutos, así que
   * `asking` no basta para cerrar el botón: sin esto se pueden lanzar dos
   * turnos sobre el mismo proyecto y pagar los dos. */
  const inFlight = asking || (running !== null && running.progress.stage !== 'done' && running.progress.stage !== 'failed');

  const load = useCallback(() => {
    setDiagrams(null);
    invoke<Diagram[]>('list_project_diagrams', { projectId })
      .then(setDiagrams)
      .catch((e) => {
        setDiagrams([]);
        toastError(String(e));
      });
  }, [projectId]);

  useEffect(load, [load]);

  const open = (fileName: string) => invoke('open_diagram', { projectId, fileName }).catch((e) => toastError(String(e)));

  /**
   * Lanza un turno headless que dibuja, o que actualiza si se le pasa la spec.
   *
   * No espera al resultado: un diagrama tarda minutos y bloquear esta pantalla
   * mientras tanto sería peor que no tener el botón. El trabajo aparece como
   * una sesión más —con su transcripción y su coste— y la lista se recarga al
   * volver a esta pantalla.
   */
  const ask = async (accountId: string, specFileName: string | null) => {
    setAsking(true);
    try {
      const sessionId = await invoke<string>('generate_project_diagram', { projectId, accountId, specFileName });
      setRunning({ sessionId, progress: IDLE });
    } catch (e) {
      toastError(String(e));
    } finally {
      setAsking(false);
    }
  };

  // Los dos eventos que el runner ya emitía y nadie miraba desde aquí: cada
  // bloque según llega, y el cambio de estado que dice que acabó.
  useEffect(() => {
    if (!running) return;
    const output = listen<{ sessionId: string; kind: string; text: string }>('session-output', (event) => {
      if (event.payload.sessionId !== running.sessionId) return;
      setRunning((prev) => (prev ? { ...prev, progress: advance(prev.progress, event.payload) } : prev));
    });
    const status = listen<[string, string]>('session-status', (event) => {
      const [sid, next] = event.payload;
      if (sid !== running.sessionId || next === 'running') return;
      // Se recarga aunque haya acabado mal: un intento fallido puede haber
      // dejado una página anterior intacta, y esconderla no ayudaría.
      setRunning((prev) => (prev ? { ...prev, progress: finish(prev.progress) } : prev));
      load();
    });
    return () => {
      output.then((f) => f());
      status.then((f) => f());
    };
  }, [running?.sessionId, load]);

  return (
    <div className={styles.field}>
      <span className={styles.fieldLabel}>DIAGRAMAS</span>
      <span className={styles.fieldHint}>
        Lo que la skill <code>archify</code> ha entregado aquí. Se guardan en los datos de Hangar, nunca dentro de tu repo, y se abren
        en tu navegador.
      </span>

      {diagrams === null ? (
        <span className={styles.empty}>leyendo…</span>
      ) : diagrams.length === 0 ? (
        <span className={styles.empty}>
          Ninguno todavía. Uno que un agente entregue en otra carpeta tampoco aparece aquí.
        </span>
      ) : (
        <div className={styles.skillList}>
          {diagrams.map((d) => (
            <div key={d.fileName} className={styles.skillRow}>
              <button className={styles.skillInfo} onClick={() => open(d.fileName)} title={`Abrir ${d.fileName}`}>
                <span className={styles.skillName}>{d.title}</span>
                <span className={styles.skillDescription}>
                  {d.generator ?? d.fileName}
                  {d.modified && ` · ${new Date(d.modified).toLocaleString('es')}`}
                  {d.bytes > 0 && ` · ${Math.round(d.bytes / 1024).toLocaleString('es')} KB`}
                  {!d.specFileName && ' · sin spec, solo se puede rehacer'}
                </span>
              </button>
              {enabled && d.specFileName && (
                <AccountPickerButton
                  label="actualizar"
                  title="Un agente compara la spec con el código de ahora y la corrige. Gasta un turno."
                  busy={inFlight}
                  disabled={inFlight}
                  onPick={(accountId) => ask(accountId, d.specFileName)}
                />
              )}
            </div>
          ))}
        </div>
      )}

      {running && (
        <div className={styles.skillList}>
          <div className={styles.skillRow}>
            <span className={styles.skillInfo}>
              <span className={styles.skillName}>
                {STAGES.map((stage, i) => {
                  const at = STAGES.indexOf(running.progress.stage);
                  const reached = running.progress.stage === 'done' || (at >= 0 && i <= at);
                  return (
                    <span key={stage} style={{ opacity: reached ? 1 : 0.35 }}>
                      {i > 0 && ' › '}
                      {STAGE_LABEL[stage]}
                      {stage === 'validating' && running.progress.validationRounds > 1 && ` (${running.progress.validationRounds})`}
                    </span>
                  );
                })}
              </span>
              <span className={styles.skillDescription}>
                {running.progress.stage === 'done'
                  ? 'Listo — la lista de abajo ya está al día.'
                  : running.progress.stage === 'failed'
                    ? 'El turno terminó sin llegar a compilar. Abre la sesión para ver dónde se quedó.'
                    : (running.progress.lastAction ?? 'esperando al agente…')}
              </span>
            </span>
          </div>
        </div>
      )}

      {!installed ? (
        // Esto sí está en otra pantalla, así que se dice dónde. Instalar es
        // elegir un repo y pasar por el preview, y eso no se ataja desde aquí.
        <span className={styles.fieldHint}>
          Para dibujar hace falta la skill <code>archify</code>, que no está en tu biblioteca. Se instala en{' '}
          <strong>Settings → Skills → repos</strong>; sale entre las sugeridas.
        </span>
      ) : !enabled ? (
        // Y esto no: la casilla está en esta misma pantalla, unos centímetros
        // más arriba. Mandar a alguien a buscarla sería un callejón con pasos
        // extra.
        <>
          <Button size="sm" variant="ghost" onClick={onEnable}>
            activar archify en este proyecto
          </Button>
          <span className={styles.fieldHint}>
            Es la misma casilla de <strong>Skills</strong>, aquí arriba. Los agentes de este proyecto la tendrán a partir de la próxima
            terminal o turno.
          </span>
        </>
      ) : (
        <>
          <AccountPickerButton
            label={diagrams && diagrams.length > 0 ? 'dibujar otro' : 'dibujar el primero'}
            title="Un agente lee el proyecto y entrega un diagrama. Gasta un turno de la cuenta que elijas."
            busy={inFlight}
            disabled={inFlight || diagrams === null}
            onPick={(accountId) => ask(accountId, null)}
          />
          <span className={styles.fieldHint}>
            Se abre como una sesión más, así que puedes seguirla y ver lo que costó. Tarda unos minutos.
          </span>
        </>
      )}
    </div>
  );
}

function CodeGraphField({ projectId, graphify }: { projectId: string; graphify: GraphifyStatus | null }) {
  const [state, setState] = useState<GraphState | null>(null);
  const [building, setBuilding] = useState(false);

  useEffect(() => {
    setState(null);
    if (!graphify?.installed) return;
    invoke<GraphState>('get_project_graph', { projectId }).then(setState).catch((e) => toastError(String(e)));
  }, [projectId, graphify?.installed]);

  const build = async (accountId: string | null) => {
    setBuilding(true);
    try {
      setState(await invoke<GraphState>('build_project_graph', { projectId, accountId }));
    } catch (e) {
      toastError(String(e));
      // El backend ya dejó el fallo escrito; releerlo es lo que hace que el
      // motivo siga ahí después de que el toast se vaya.
      invoke<GraphState>('get_project_graph', { projectId }).then(setState).catch(() => {});
    } finally {
      setBuilding(false);
    }
  };

  return (
    <div className={styles.field}>
      <span className={styles.fieldLabel}>GRAFO DE CÓDIGO</span>
      <span className={styles.fieldHint}>
        Un mapa del repo que los agentes consultan en vez de reconstruirlo a base de <code>grep</code> en cada turno. Se indexa{' '}
        <strong>solo código, en local</strong>, y se escribe en los datos de Hangar — nunca dentro de tu repo.
      </span>

      {!graphify?.installed ? (
        // La pista concreta vive en Settings → general, que es lo único que
        // sabe si el prerrequisito (uv) también falta. Repetir aquí un comando
        // que puede no ser ejecutable es justo el callejón sin salida que esa
        // pantalla existe para evitar.
        <span className={styles.empty}>
          Necesita <strong>graphify</strong>, que no está en esta máquina. En <strong>Settings → general</strong> tienes el paso que
          falta. Sin él, todo lo demás funciona igual.
        </span>
      ) : !graphify.mcpInstalled ? (
        <span className={styles.empty}>
          Está <code>graphify</code> pero falta <code>graphify-mcp</code>, que es lo que se lo sirve a los agentes. Reinstala con{' '}
          <code>{graphify.installHint}</code>.
        </span>
      ) : (
        <>
          <div className={styles.skillList}>
            {state?.status === 'ready' ? (
              <span className={styles.skillDescription}>
                {state.nodeCount.toLocaleString('es')} nodos · {state.edgeCount.toLocaleString('es')} aristas
                {state.builtAt && ` · construido ${new Date(state.builtAt).toLocaleString('es')}`}
              </span>
            ) : state?.status === 'failed' ? (
              <span className={styles.skillDescription}>La última construcción falló: {state.lastError ?? 'sin detalle'}</span>
            ) : (
              <span className={styles.skillDescription}>Todavía sin construir. Tarda unos segundos en un repo mediano.</span>
            )}

            {/* §5.5: la antigüedad se dice, no se deduce. Y se dice por
                miembro, porque un aviso que solo dice "viejo" no dice qué
                reconstruir. */}
            {state?.stale && (
              <span className={styles.skillDescription}>
                ⚠ El repo ha avanzado desde que se construyó. Los agentes lo siguen usando, pero se les dice la edad con cada respuesta.
              </span>
            )}
          </div>

          <div className={styles.skillList}>
            {state?.withDocs ? (
              // Indexar documentos ejecuta `claude` por cada fichero, así que
              // gasta cuota de una cuenta concreta. Se elige al pedirlo, igual
              // que en los diagramas, en vez de en un ajuste que se olvida.
              <AccountPickerButton
                label={building ? 'construyendo…' : 'construir con…'}
                title="Indexar documentos gasta cuota de la cuenta que elijas."
                busy={building}
                disabled={building}
                onPick={(accountId) => build(accountId)}
              />
            ) : (
              <button className={styles.select} disabled={building} onClick={() => build(null)}>
                {building ? <Spinner label="indexando…" /> : state?.status === 'ready' ? 'Reconstruir el grafo' : 'Construir el grafo'}
              </button>
            )}
            {state?.htmlPath && (
              <button
                className={styles.select}
                onClick={() => invoke('open_graph_html', { projectId }).catch((e) => toastError(String(e)))}
              >
                Abrir la vista de graphify en el navegador
              </button>
            )}
          </div>
        </>
      )}
    </div>
  );
}

/** Un desplegable con "hereda" como primera opción y valor real.
 *
 * "Hereda" nombra lo que heredaría, en vez de dejarlo en blanco: sin eso hay
 * que ir a General y volver para saber qué está pasando de verdad. */
function LanguageField({
  label,
  hint,
  value,
  inherited,
  disabled,
  onChange,
}: {
  label: string;
  hint: string;
  value: string | null;
  inherited: string | null;
  disabled?: boolean;
  onChange: (value: string | null) => void;
}) {
  return (
    <div className={styles.field}>
      <span className={styles.fieldLabel}>{label}</span>
      <span className={styles.fieldHint}>{hint}</span>
      <select
        className={styles.select}
        value={value ?? INHERIT}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value === INHERIT ? null : e.target.value)}
      >
        <option value={INHERIT}>{inherited ? `heredar de General — ${inherited}` : 'heredar de General — sin preferencia'}</option>
        {LANGUAGES.map((l) => (
          <option key={l.code} value={l.label}>
            {l.label}
          </option>
        ))}
      </select>
    </div>
  );
}
