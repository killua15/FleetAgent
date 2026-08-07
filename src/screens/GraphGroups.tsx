import { useCallback, useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { SettingsTabs } from '../components/SettingsTabs';
import { toastError } from '../components/ErrorOverlay';
import { Spinner } from '../components/Spinner';
import { SeamMap } from '../components/SeamMap';
import type { Project } from '../types/session';
import type { Bridge, GraphGroup, GraphSavingsReport, GraphState, GraphifyStatus } from '../types/graph';
import styles from './GraphGroups.module.css';

/**
 * Proyectos que son un mismo sistema, indexados en un grafo común.
 *
 * Misma forma de tres columnas que `SharedMemorySettings`, porque es la misma
 * pregunta con otra respuesta: qué proyectos van juntos. Lo que cambia es la
 * tercera columna — allí son ficheros de memoria, aquí son **los puentes**, que
 * son la prueba de que agrupar sirvió de algo.
 *
 * El recuento de puentes se enseña siempre, **cero incluido**: un grupo que no
 * aportó nada tiene que verse en la lista, no descubrirse abriendo el grafo.
 */
export function GraphGroups() {
  const [graphify, setGraphify] = useState<GraphifyStatus | null>(null);
  const [groups, setGroups] = useState<GraphGroup[]>([]);
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [members, setMembers] = useState<string[]>([]);
  const [state, setState] = useState<GraphState | null>(null);
  const [bridges, setBridges] = useState<Bridge[]>([]);
  const [savings, setSavings] = useState<GraphSavingsReport | null>(null);
  const [newName, setNewName] = useState('');
  const [building, setBuilding] = useState(false);

  const reloadGroups = useCallback(
    () => invoke<GraphGroup[]>('list_graph_groups').then(setGroups).catch((e) => toastError(String(e))),
    [],
  );

  useEffect(() => {
    Promise.all([
      invoke<GraphifyStatus>('graphify_status').then(setGraphify),
      invoke<Project[]>('list_projects').then(setProjects),
      reloadGroups(),
    ]).catch((e) => toastError(String(e)));
  }, [reloadGroups]);

  useEffect(() => {
    if (!selectedId) return;
    setSavings(null);
    Promise.all([
      invoke<string[]>('get_graph_group_members', { groupId: selectedId }).then(setMembers),
      invoke<GraphState>('get_graph_group_state', { groupId: selectedId }).then(setState),
      invoke<Bridge[]>('get_graph_bridges', { groupId: selectedId }).then(setBridges),
    ]).catch((e) => toastError(String(e)));
  }, [selectedId]);

  const createGroup = async () => {
    if (!newName.trim()) return;
    try {
      const created = await invoke<GraphGroup>('create_graph_group', { name: newName.trim() });
      setNewName('');
      await reloadGroups();
      setSelectedId(created.id);
    } catch (e) {
      toastError(String(e));
    }
  };

  const toggleMember = async (projectId: string, member: boolean) => {
    if (!selectedId) return;
    const previous = members;
    setMembers((prev) => (member ? [...prev, projectId] : prev.filter((id) => id !== projectId)));
    try {
      await invoke('set_graph_group_member', { groupId: selectedId, projectId, member });
      // Cambiar los miembros invalida el recuento: un número viejo al lado de
      // una pertenencia nueva es peor que ningún número.
      await reloadGroups();
      setBridges([]);
    } catch (e) {
      setMembers(previous);
      toastError(String(e));
    }
  };

  const build = async () => {
    if (!selectedId) return;
    setBuilding(true);
    try {
      setState(await invoke<GraphState>('build_graph_group', { groupId: selectedId }));
      setBridges(await invoke<Bridge[]>('get_graph_bridges', { groupId: selectedId }));
      await reloadGroups();
    } catch (e) {
      toastError(String(e));
      invoke<GraphState>('get_graph_group_state', { groupId: selectedId }).then(setState).catch(() => {});
    } finally {
      setBuilding(false);
    }
  };

  const selected = groups.find((g) => g.id === selectedId);
  const memberProjects = projects.filter((p) => members.includes(p.id));

  return (
    <div className={styles.screen}>
      <SettingsTabs
        active="graph-groups"
        title="Graph groups"
        subtitle="proyectos que son un mismo sistema: se indexan en un grafo común para que un agente en uno vea el otro lado."
      />

      {!graphify?.installed ? (
        <div className={styles.missing}>
          Necesita <strong>graphify</strong>, que no está en esta máquina. Se instala con <code>{graphify?.installHint ?? "uv tool install 'graphifyy[sql]'"}</code>.
          Sin él, Hangar funciona exactamente igual que hasta ahora.
        </div>
      ) : (
        <div className={styles.body}>
          <div className={styles.groupList}>
            <span className={styles.sectionLabel}>GRUPOS</span>
            {groups.map((g) => (
              <button
                key={g.id}
                className={g.id === selectedId ? styles.groupRowActive : styles.groupRow}
                onClick={() => setSelectedId(g.id)}
              >
                <span className={styles.groupDot} />
                <span className={styles.groupInfo}>
                  <span className={styles.groupName}>{g.name}</span>
                  <span className={styles.groupSub}>
                    {g.projectCount} proyecto{g.projectCount === 1 ? '' : 's'} ·{' '}
                    {g.bridgeCount === null ? 'sin construir' : `${g.bridgeCount} puente${g.bridgeCount === 1 ? '' : 's'}`}
                  </span>
                </span>
              </button>
            ))}
            {groups.length === 0 && <span className={styles.empty}>Ningún grupo todavía.</span>}
            <div className={styles.newGroupRow}>
              <input
                className={styles.input}
                value={newName}
                onChange={(e) => setNewName(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && createGroup()}
                placeholder="nombre del grupo…"
              />
              <button className={styles.btn} onClick={createGroup} title="Nuevo grupo">
                +
              </button>
            </div>
            <div className={styles.spacer} />
            <p className={styles.footnote}>
              Construir un grupo <strong>no escribe nada</strong> dentro de tus repos: el índice vive en los datos de Hangar.
            </p>
          </div>

          <div className={styles.membership}>
            {selected ? (
              <>
                <span className={styles.membershipTitle}>{selected.name}</span>
                <span className={styles.membershipHint}>Proyectos que se indexan juntos:</span>
                {projects.map((p) => (
                  <label key={p.id} className={members.includes(p.id) ? styles.memberRowActive : styles.memberRow}>
                    <input type="checkbox" checked={members.includes(p.id)} onChange={(e) => toggleMember(p.id, e.target.checked)} />
                    <span className={styles.memberIcon}>{p.name[0]?.toUpperCase()}</span>
                    <span className={styles.memberInfo}>
                      <span className={styles.memberName}>{p.name}</span>
                      {/* La frescura se enseña por miembro, no solo por grupo:
                          un aviso que solo dice "viejo" no dice qué reconstruir. */}
                      <span className={styles.memberSha}>
                        {state?.commits.find((c) => c.projectId === p.id)?.stale
                          ? 'ha avanzado desde la construcción'
                          : state?.commits.find((c) => c.projectId === p.id)?.builtSha.slice(0, 7) ?? ''}
                      </span>
                    </span>
                  </label>
                ))}

                {members.length < 2 && (
                  <div className={styles.note}>Un grupo necesita al menos dos proyectos — es lo que significa agrupar.</div>
                )}

                {state?.stale && (
                  <div className={styles.warn}>
                    ⚠ Este grafo está viejo: algún miembro ha avanzado. Los agentes lo siguen usando, pero se les dice la edad con
                    cada respuesta.
                  </div>
                )}
                {state?.status === 'failed' && <div className={styles.warn}>La última construcción falló: {state.lastError}</div>}

                <button className={styles.btnPrimary} disabled={building || members.length < 2} onClick={build}>
                  {building ? <Spinner label="indexando y fusionando…" /> : state?.status === 'ready' ? 'Reconstruir el grafo' : 'Construir el grafo'}
                </button>

                <SavingsPanel groupId={selected.id} memberProjects={memberProjects} savings={savings} onLoad={setSavings} />
              </>
            ) : (
              <span className={styles.empty}>Elige un grupo, o crea uno.</span>
            )}
          </div>

          <div className={styles.bridgeWrap}>
            <span className={styles.sectionLabel}>PUENTES — LO QUE DE VERDAD CRUZA</span>
            <span className={styles.bridgeHint}>
              El mismo nombre de tipo a los dos lados. <strong>Esto es el resultado:</strong> si sale vacío, agrupar no te dio nada y
              conviene deshacerlo.
            </span>
            {state?.status !== 'ready' ? (
              <span className={styles.empty}>Construye el grafo para ver la costura.</span>
            ) : bridges.length === 0 ? (
              <div className={styles.warn}>
                Cero puentes. Estos proyectos no comparten ni un nombre de tipo — son dos islas, y dibujarlas juntas no las convierte
                en un sistema.
              </div>
            ) : (
              <SeamMap bridges={bridges} />
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * El ahorro medido (Fase 3, Diferencial B).
 *
 * Se pide por proyecto porque `turn_usage` cuelga de un proyecto, no de un
 * grupo. Y se presenta con el tamaño de muestra delante: un informe que solo
 * sabe dar buenas noticias no es un informe.
 */
function SavingsPanel({
  memberProjects,
  savings,
  onLoad,
}: {
  groupId: string;
  memberProjects: Project[];
  savings: GraphSavingsReport | null;
  onLoad: (r: GraphSavingsReport) => void;
}) {
  const first = memberProjects[0];
  if (!first) return null;

  return (
    <div className={styles.savings}>
      <span className={styles.sectionLabel}>AHORRO MEDIDO — {first.name.toUpperCase()}</span>
      {!savings ? (
        <button
          className={styles.btn}
          onClick={() =>
            invoke<GraphSavingsReport>('graph_savings_report', { projectId: first.id }).then(onLoad).catch((e) => toastError(String(e)))
          }
        >
          Comparar turnos con y sin grafo
        </button>
      ) : (
        <>
          <div className={styles.savingsRow}>
            <span>Con grafo</span>
            <span className={styles.num}>
              {savings.withGraph.turns} turnos · {Math.round(savings.withGraph.avgInputTokens).toLocaleString('es')} tok entrada ·{' '}
              ${savings.withGraph.avgCostUsd.toFixed(4)}
            </span>
          </div>
          <div className={styles.savingsRow}>
            <span>Sin grafo</span>
            <span className={styles.num}>
              {savings.withoutGraph.turns} turnos · {Math.round(savings.withoutGraph.avgInputTokens).toLocaleString('es')} tok entrada ·{' '}
              ${savings.withoutGraph.avgCostUsd.toFixed(4)}
            </span>
          </div>
          {savings.unattributedTurns > 0 && (
            <span className={styles.empty}>
              {savings.unattributedTurns} turnos antiguos no se pueden atribuir a ningún lado (se guardaron sin proyecto). No entran
              en la comparación.
            </span>
          )}
          {savings.inputDelta === null ? (
            <span className={styles.empty}>Todavía no hay turnos a los dos lados de la línea. No hay nada que comparar.</span>
          ) : savings.tooEarly ? (
            <div className={styles.note}>
              La diferencia sale{' '}
              <strong>
                {savings.inputDelta > 0 ? '−' : '+'}
                {Math.abs(Math.round(savings.inputDelta)).toLocaleString('es')} tokens
              </strong>{' '}
              por turno, pero con esta muestra eso es ruido, no una conclusión. Hacen falta más turnos a los dos lados.
            </div>
          ) : (
            <div className={styles.note}>
              {savings.inputDelta > 0 ? (
                <>
                  El grafo ahorra <strong>{Math.round(savings.inputDelta).toLocaleString('es')} tokens de entrada</strong> por turno,{' '}
                  <strong>${Math.abs(savings.costDeltaUsd ?? 0).toFixed(4)}</strong> de media.
                </>
              ) : (
                <>
                  El grafo <strong>cuesta {Math.abs(Math.round(savings.inputDelta)).toLocaleString('es')} tokens más</strong> por turno.
                  Eso es lo que dicen tus datos.
                </>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}
