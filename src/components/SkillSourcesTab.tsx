import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { toastError } from './ErrorOverlay';
import { ConfirmDeleteDialog } from './ConfirmDeleteDialog';
import { Button } from './ui/button';
import type { RemoteSkill, SkillSource, SuggestedSource } from '../types/settings';
import styles from '../screens/SkillSettings.module.css';

/**
 * Instalar skills desde repositorios de GitHub.
 *
 * Todo lo hace `gh skill`, que ya es dependencia de Hangar para PRs e issues:
 * no hay descarga propia, ni resolución de versiones, ni seguimiento de origen.
 * Un repo privado funciona igual, porque `gh` habla autenticado como tú — que
 * es el caso de un repo de skills de equipo.
 *
 * **El preview es obligatorio y no es un adorno.** Una skill parece
 * documentación —es un markdown— pero puede pre-autorizar la ejecución de sus
 * propios scripts con `allowed-tools`. El marketplace de JetBrains revisa y
 * firma; GitHub no revisa nada. Esta pantalla es lo único que hay entre tú y esa
 * diferencia, así que no existe ningún camino que instale sin pasar por ella.
 */
export function SkillSourcesTab({ onInstalled }: { onInstalled: () => void }) {
  const [sources, setSources] = useState<SkillSource[]>([]);
  const [suggested, setSuggested] = useState<SuggestedSource[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [remote, setRemote] = useState<RemoteSkill[]>([]);
  const [loading, setLoading] = useState(false);
  const [input, setInput] = useState('');
  const [adding, setAdding] = useState(false);
  // Un repo puede traer cientos de skills — `github/awesome-copilot` tiene 411.
  // Sin filtro la lista no se puede usar.
  const [filter, setFilter] = useState('');
  const [preview, setPreview] = useState<{ skill: string; body: string } | null>(null);
  const [installing, setInstalling] = useState(false);

  const loadSources = () => invoke<SkillSource[]>('list_skill_sources').then(setSources).catch((e) => toastError(String(e)));

  useEffect(() => {
    loadSources();
    invoke<SuggestedSource[]>('suggested_skill_sources').then(setSuggested).catch(() => {});
  }, []);

  const openSource = async (repo: string) => {
    setSelected(repo);
    setRemote([]);
    setFilter('');
    setLoading(true);
    try {
      setRemote(await invoke<RemoteSkill[]>('list_source_skills', { repo }));
    } catch (e) {
      toastError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const add = async (repo?: string, label?: string) => {
    const wanted = (repo ?? input).trim();
    if (!wanted) return;
    setAdding(true);
    try {
      const source = await invoke<SkillSource>('add_skill_source', { repo: wanted, label: label ?? null });
      setInput('');
      await loadSources();
      openSource(source.repo);
    } catch (e) {
      toastError(String(e));
    } finally {
      setAdding(false);
    }
  };

  const openPreview = async (skill: string) => {
    if (!selected) return;
    setPreview({ skill, body: 'leyendo…' });
    try {
      setPreview({ skill, body: await invoke<string>('preview_source_skill', { repo: selected, skill }) });
    } catch (e) {
      setPreview(null);
      toastError(String(e));
    }
  };

  const install = async () => {
    if (!selected || !preview) return;
    setInstalling(true);
    try {
      await invoke('install_source_skill', { repo: selected, skill: preview.skill });
      setPreview(null);
      await openSource(selected);
      onInstalled();
    } catch (e) {
      toastError(String(e));
    } finally {
      setInstalling(false);
    }
  };

  const unadded = suggested.filter((s) => !sources.some((existing) => existing.repo === s.repo));

  const shown = remote.filter(
    (s) => !filter.trim() || s.name.includes(filter.trim()) || s.description.toLowerCase().includes(filter.trim().toLowerCase()),
  );

  return (
    <div className={styles.body}>
      <div className={styles.list}>
        <div className={styles.listHead}>
          <span className={styles.sectionLabel}>REPOSITORIOS</span>
        </div>

        <div className={styles.addRow}>
          <input
            className={styles.input}
            placeholder="owner/repo o su URL"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            onKeyDown={(e) => e.key === 'Enter' && add()}
          />
          <Button size="sm" disabled={adding || !input.trim()} onClick={() => add()}>
            {adding ? '…' : 'añadir'}
          </Button>
        </div>

        {sources.length === 0 && (
          <span className={styles.empty}>
            Ninguno todavía. Añade el repo de skills de tu equipo — si es privado también funciona, porque Hangar usa tu sesión de{' '}
            <code>gh</code>.
          </span>
        )}

        {/* Pedir `owner/repo` a pelo es una pregunta que solo puede contestar
         * quien ya sabe la respuesta. Se ofrecen los que aún no están, así que
         * la sección se vacía sola a medida que se usan. */}
        {unadded.length > 0 && (
          <>
            <div className={styles.listHead}>
              <span className={styles.sectionLabel}>PARA EMPEZAR</span>
            </div>
            {unadded.map((s) => (
              <div key={s.repo} className={styles.row}>
                <span className={styles.rowMain}>
                  <span className={styles.rowName}>{s.label}</span>
                  <span className={styles.rowDescription}>{s.blurb}</span>
                </span>
                <Button size="sm" variant="ghost" disabled={adding} onClick={() => add(s.repo, s.label)}>
                  añadir
                </Button>
              </div>
            ))}
          </>
        )}

        {sources.map((source) => (
          <div key={source.id} className={selected === source.repo ? styles.rowActive : styles.row}>
            <button className={styles.rowMain} onClick={() => openSource(source.repo)}>
              <span className={styles.rowName}>{source.label}</span>
              <span className={styles.rowDescription}>{source.repo}</span>
            </button>
            <ConfirmDeleteDialog
              trigger={
                <button className={styles.iconBtn} title="Quitar repositorio">
                  🗑
                </button>
              }
              title={`¿Quitar ${source.repo}?`}
              description="Solo se quita de la lista. Las skills que ya instalaste desde él se quedan en tu biblioteca — son tuyas desde que aterrizaron."
              onConfirm={() => remove(source.id, loadSources, selected, setSelected)}
            />
          </div>
        ))}
      </div>

      <div className={styles.editor}>
        {!selected && <span className={styles.empty}>Elige un repositorio para ver qué skills trae.</span>}

        {selected && !preview && (
          <>
            <div className={styles.panelHead}>
              <span className={styles.fieldLabel}>{selected}</span>
              <span className={styles.spacer} />
              <span className={styles.fieldHint}>{loading ? 'leyendo…' : `${shown.length} de ${remote.length}`}</span>
            </div>

            {remote.length > 0 && (
              <input
                className={styles.input}
                placeholder="filtrar por nombre o descripción…"
                value={filter}
                onChange={(e) => setFilter(e.target.value)}
              />
            )}

            <div className={styles.remoteList}>
              {shown.map((skill) => (
                <div key={skill.name} className={styles.row}>
                  <span className={styles.rowMain}>
                    <span className={styles.rowName}>
                      /{skill.name}
                      {skill.installed && <span className={styles.badge}>ya instalada</span>}
                    </span>
                    <span className={styles.rowDescription}>{skill.description}</span>
                  </span>
                  {/* Nunca «instalar» directo: el botón lleva al preview, y la
                      instalación vive dentro de esa vista. */}
                  <Button size="sm" variant="outline" onClick={() => openPreview(skill.name)}>
                    ver
                  </Button>
                </div>
              ))}
              {!loading && remote.length === 0 && <span className={styles.empty}>Este repositorio no expone ninguna skill.</span>}
            </div>
          </>
        )}

        {preview && (
          <>
            <div className={styles.panelHead}>
              <span className={styles.fieldLabel}>/{preview.skill}</span>
              <span className={styles.spacer} />
              <Button size="sm" variant="outline" onClick={() => setPreview(null)}>
                volver
              </Button>
              <Button size="sm" disabled={installing} onClick={install}>
                {installing ? 'instalando…' : 'Instalar'}
              </Button>
            </div>
            <span className={styles.fieldHint}>
              Mira qué trae antes de instalarla. Si incluye una carpeta <code>scripts/</code> o un campo{' '}
              <code>allowed-tools</code>, puede ejecutar cosas sin pedirte permiso. GitHub no revisa lo que se publica.
            </span>
            <pre className={styles.preview}>{preview.body}</pre>
          </>
        )}
      </div>
    </div>
  );
}

async function remove(
  id: string,
  reload: () => Promise<void>,
  selected: string | null,
  setSelected: (v: string | null) => void,
) {
  try {
    await invoke('remove_skill_source', { id });
    await reload();
    if (selected) setSelected(null);
  } catch (e) {
    toastError(String(e));
  }
}
