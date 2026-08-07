import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { SettingsTabs } from '../components/SettingsTabs';
import { toastError } from '../components/ErrorOverlay';
import { Spinner } from '../components/Spinner';
import { LANGUAGES } from '../lib/languages';
import type { Project } from '../types/session';
import type { AgentPreferences, Skill } from '../types/settings';
import type { GraphState, GraphifyStatus } from '../types/graph';
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
function CodeGraphField({ projectId, graphify }: { projectId: string; graphify: GraphifyStatus | null }) {
  const [state, setState] = useState<GraphState | null>(null);
  const [building, setBuilding] = useState(false);

  useEffect(() => {
    setState(null);
    if (!graphify?.installed) return;
    invoke<GraphState>('get_project_graph', { projectId }).then(setState).catch((e) => toastError(String(e)));
  }, [projectId, graphify?.installed]);

  const build = async () => {
    setBuilding(true);
    try {
      setState(await invoke<GraphState>('build_project_graph', { projectId }));
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
            <button className={styles.select} disabled={building} onClick={build}>
              {building ? <Spinner label="indexando…" /> : state?.status === 'ready' ? 'Reconstruir el grafo' : 'Construir el grafo'}
            </button>
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
