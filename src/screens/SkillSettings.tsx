import { useEffect, useState } from 'react';
import { invoke } from '../lib/invoke';
import { SettingsTabs } from '../components/SettingsTabs';
import { ConfirmDeleteDialog } from '../components/ConfirmDeleteDialog';
import { toastError } from '../components/ErrorOverlay';
import { Button } from '../components/ui/button';
import type { Skill } from '../types/settings';
import styles from './SkillSettings.module.css';

/**
 * Tu biblioteca de skills.
 *
 * Una skill es una carpeta con un `SKILL.md` — formato abierto, creado por
 * Anthropic y liberado, que leen igual Claude Code, Codex, Gemini CLI y varias
 * decenas de clientes más. Hangar no inventa formato ni convierte entre unos y
 * otros: guarda las carpetas y apunta cada CLI a la misma.
 *
 * Viven en disco, no en la base de datos, porque el estándar dice que una skill
 * es una carpeta y puede traer `scripts/`, `references/` y `assets/`. Guardar
 * solo el texto en una columna cerraría esa puerta el primer día — de ahí el
 * botón para abrirla en Finder en vez de construir un explorador de ficheros.
 *
 * Cuáles se activan en cada proyecto se decide en la pestaña de proyectos.
 */
const BLANK: Skill = { name: '', description: '', body: '', hasExtraFiles: false };

export function SkillSettings() {
  const [skills, setSkills] = useState<Skill[]>([]);
  const [editing, setEditing] = useState<Skill | null>(null);
  const [isNew, setIsNew] = useState(false);
  const [saving, setSaving] = useState(false);

  const load = () => invoke<Skill[]>('list_skills').then(setSkills).catch((e) => toastError(String(e)));

  useEffect(() => {
    load();
  }, []);

  const save = async () => {
    if (!editing) return;
    setSaving(true);
    try {
      await invoke<Skill>('save_skill', { input: editing });
      await load();
      setEditing(null);
      setIsNew(false);
    } catch (e) {
      toastError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (name: string) => {
    try {
      await invoke('delete_skill', { name });
      await load();
      if (editing?.name === name) setEditing(null);
    } catch (e) {
      toastError(String(e));
    }
  };

  const reveal = async (name: string) => {
    try {
      await invoke<string>('reveal_skill_directory', { name });
    } catch (e) {
      toastError(String(e));
    }
  };

  return (
    <div className={styles.screen}>
      <SettingsTabs
        active="skill-settings"
        title="Skills"
        subtitle="carpetas con un SKILL.md, en el formato abierto que leen todos los CLI. Cuáles se activan se elige por proyecto."
      />
      <div className={styles.body}>
        <div className={styles.list}>
          <div className={styles.listHead}>
            <span className={styles.sectionLabel}>BIBLIOTECA</span>
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                setEditing({ ...BLANK });
                setIsNew(true);
              }}
            >
              + nueva
            </Button>
          </div>

          {skills.length === 0 && (
            <span className={styles.empty}>
              Ninguna todavía. Una skill es un procedimiento que repites — cómo desplegar, cómo revisar un PR, cómo arrancar este
              proyecto — escrito una vez para que el agente lo siga igual siempre.
            </span>
          )}

          {skills.map((skill) => (
            <div key={skill.name} className={editing?.name === skill.name && !isNew ? styles.rowActive : styles.row}>
              <button
                className={styles.rowMain}
                onClick={() => {
                  setEditing(skill);
                  setIsNew(false);
                }}
              >
                <span className={styles.rowName}>
                  /{skill.name}
                  {skill.hasExtraFiles && <span className={styles.badge}>+ ficheros</span>}
                </span>
                <span className={styles.rowDescription}>{skill.description || 'sin descripción'}</span>
              </button>
              <ConfirmDeleteDialog
                trigger={
                  <button className={styles.iconBtn} title="Borrar skill">
                    🗑
                  </button>
                }
                title={`¿Borrar /${skill.name}?`}
                description="Se borra la carpeta entera, incluidos los ficheros que tenga dentro, y deja de estar activa en todos los proyectos. No se puede deshacer."
                onConfirm={() => remove(skill.name)}
              />
            </div>
          ))}
        </div>

        <div className={styles.editor}>
          {!editing && <span className={styles.empty}>Elige una skill para editarla, o crea una nueva.</span>}
          {editing && (
            <>
              <label className={styles.field}>
                <span className={styles.fieldLabel}>NOMBRE</span>
                <span className={styles.fieldHint}>
                  Es la carpeta y también el comando: <code>/{editing.name || 'nombre'}</code>. Letras, números, guiones.
                </span>
                <input
                  className={styles.input}
                  value={editing.name}
                  disabled={!isNew}
                  placeholder="revisar-pr"
                  onChange={(e) => setEditing({ ...editing, name: e.target.value })}
                />
                {!isNew && <span className={styles.fieldHint}>El nombre no se cambia: es la carpeta y lo que los proyectos tienen marcado.</span>}
              </label>

              <label className={styles.field}>
                <span className={styles.fieldLabel}>CUÁNDO USARLA</span>
                {/* Lo más importante del formulario y por eso se explica en vez
                    de tratarse como una etiqueta: hasta que el agente decide
                    abrir la skill, esto es lo ÚNICO que ve de ella. */}
                <span className={styles.fieldHint}>
                  Es lo único que el agente ve hasta que decide abrirla, así que de esto depende que se active o no. Di cuándo
                  aplica y cuándo no, no lo que hace.
                </span>
                <textarea
                  className={styles.textarea}
                  rows={3}
                  value={editing.description}
                  placeholder="Usar al revisar un pull request de este equipo. No usar para revisar código sin PR."
                  onChange={(e) => setEditing({ ...editing, description: e.target.value })}
                />
              </label>

              <label className={styles.field}>
                <span className={styles.fieldLabel}>INSTRUCCIONES</span>
                <span className={styles.fieldHint}>Se cargan solo cuando la skill se activa, así que pueden ser largas sin coste.</span>
                <textarea
                  className={styles.textareaBig}
                  rows={16}
                  value={editing.body}
                  placeholder={'1. Lee el diff completo antes de opinar.\n2. Comprueba que hay tests.\n3. …'}
                  onChange={(e) => setEditing({ ...editing, body: e.target.value })}
                />
              </label>

              <div className={styles.actions}>
                {!isNew && (
                  <Button size="sm" variant="outline" onClick={() => reveal(editing.name)}>
                    abrir carpeta
                  </Button>
                )}
                <span className={styles.spacer} />
                <Button size="sm" variant="outline" onClick={() => setEditing(null)}>
                  cancelar
                </Button>
                <Button size="sm" disabled={saving || !editing.name.trim()} onClick={save}>
                  {saving ? 'guardando…' : 'Guardar'}
                </Button>
              </div>

              <p className={styles.footnote}>
                Para añadirle <code>scripts/</code> o <code>references/</code>, abre la carpeta y déjalos dentro: el estándar los
                admite y el CLI los encuentra solo.
              </p>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
