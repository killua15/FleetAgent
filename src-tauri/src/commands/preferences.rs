//! What the user wants every agent to know, before they type anything.
//!
//! The gap this closes is the one `SettingsTabs.tsx` documents about shared
//! memory: Hangar had settings, and none of them ever reached the CLI. Asking
//! for Spanish and getting English back, every session, is that gap in its
//! most visible form.
//!
//! **A terminal has no prompt to prepend to.** The user types into a live PTY,
//! so "put the language in front of the first message" cannot work for the case
//! that motivates this — there is no first message Hangar owns. The only door
//! is a flag at spawn time, which is why this module's whole output is a file
//! path handed to [`crate::providers::instruction_args`].

use crate::db::DbConnection;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::State;

/// Global defaults, in the same `app_settings` key/value table the terminal
/// mode uses (`bridge::session`). A whole struct in `AppSettings` would mean
/// touching three places in `commands::settings` for two strings.
const CONVERSATION_LANGUAGE_KEY: &str = "prompt.conversation_language";
const COMMENT_LANGUAGE_KEY: &str = "prompt.comment_language";

/// One layer of preferences — global or a project's override.
///
/// `None` means "not set here". For a project row that reads as *inherit*; for
/// the global layer it reads as *nothing to say*, which is what keeps the flag
/// off the command line entirely rather than passing an empty file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentPreferences {
    /// What the agent talks to you in.
    pub conversation_language: Option<String>,
    /// What it writes *into the code* — comments, identifiers, commit
    /// messages. Separate from the above on purpose: wanting Spanish replies
    /// on an English codebase is the common case, not an edge one.
    pub comment_language: Option<String>,
}

impl AgentPreferences {
    fn is_empty(&self) -> bool {
        self.conversation_language.is_none() && self.comment_language.is_none()
    }

    /// `self` wins field by field, falling back to `base`. Field-by-field
    /// rather than all-or-nothing: overriding only the comment language must
    /// not silently drop the global conversation language.
    fn over(self, base: &AgentPreferences) -> AgentPreferences {
        AgentPreferences {
            conversation_language: self.conversation_language.or_else(|| base.conversation_language.clone()),
            comment_language: self.comment_language.or_else(|| base.comment_language.clone()),
        }
    }
}

/// Empty strings are how a `<select>` says "none", and they must not become a
/// stored `""` that later renders as an instruction with a blank language.
fn clean(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

// ------------------------------------------------------------------ global

pub(crate) fn read_global(conn: &rusqlite::Connection) -> Result<AgentPreferences, String> {
    let get = |key: &str| -> Result<Option<String>, String> {
        conn.query_row("SELECT value FROM app_settings WHERE key = ?1", params![key], |row| row.get::<_, String>(0))
            .optional()
            .map_err(|e| e.to_string())
            .map(clean)
    };
    Ok(AgentPreferences {
        conversation_language: get(CONVERSATION_LANGUAGE_KEY)?,
        comment_language: get(COMMENT_LANGUAGE_KEY)?,
    })
}

#[tauri::command]
pub fn get_global_agent_preferences(db: State<DbConnection>) -> Result<AgentPreferences, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    read_global(&conn)
}

#[tauri::command]
pub fn set_global_agent_preferences(db: State<DbConnection>, input: AgentPreferences) -> Result<AgentPreferences, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let input = AgentPreferences {
        conversation_language: clean(input.conversation_language),
        comment_language: clean(input.comment_language),
    };
    // Deleting rather than storing `""` keeps "never set" and "set to nothing"
    // the same row-level fact, so `read_global` needs no second rule.
    let write = |key: &str, value: &Option<String>| -> Result<(), String> {
        match value {
            Some(v) => conn.execute(
                "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![key, v],
            ),
            None => conn.execute("DELETE FROM app_settings WHERE key = ?1", params![key]),
        }
        .map(|_| ())
        .map_err(|e| e.to_string())
    };
    write(CONVERSATION_LANGUAGE_KEY, &input.conversation_language)?;
    write(COMMENT_LANGUAGE_KEY, &input.comment_language)?;
    Ok(input)
}

// ----------------------------------------------------------- per project

pub(crate) fn read_project(conn: &rusqlite::Connection, project_id: &str) -> Result<AgentPreferences, String> {
    conn.query_row(
        "SELECT conversation_language, comment_language FROM project_preferences WHERE project_id = ?1",
        params![project_id],
        |row| {
            Ok(AgentPreferences {
                conversation_language: row.get(0)?,
                comment_language: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(|e| e.to_string())
    // No row and a row of all-NULLs mean the same thing — inherit everything —
    // so they must not be distinguishable to callers.
    .map(|found| found.unwrap_or_default())
    .map(|prefs| AgentPreferences {
        conversation_language: clean(prefs.conversation_language),
        comment_language: clean(prefs.comment_language),
    })
}

#[tauri::command]
pub fn get_project_agent_preferences(db: State<DbConnection>, project_id: String) -> Result<AgentPreferences, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    read_project(&conn, &project_id)
}

#[tauri::command]
pub fn set_project_agent_preferences(db: State<DbConnection>, project_id: String, input: AgentPreferences) -> Result<AgentPreferences, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let input = AgentPreferences {
        conversation_language: clean(input.conversation_language),
        comment_language: clean(input.comment_language),
    };
    conn.execute(
        "INSERT INTO project_preferences (project_id, conversation_language, comment_language)
         VALUES (?1, ?2, ?3)
         ON CONFLICT(project_id) DO UPDATE SET
           conversation_language = excluded.conversation_language,
           comment_language = excluded.comment_language",
        params![project_id, input.conversation_language, input.comment_language],
    )
    .map_err(|e| e.to_string())?;
    Ok(input)
}

// -------------------------------------------------------------- resolution

/// What actually applies to a session in `project_id`: the project's own
/// values where it has them, the global defaults everywhere else.
pub fn resolve(conn: &rusqlite::Connection, project_id: Option<&str>) -> Result<AgentPreferences, String> {
    let global = read_global(conn)?;
    match project_id {
        Some(id) => Ok(read_project(conn, id)?.over(&global)),
        None => Ok(global),
    }
}

/// The system-prompt text, or `None` when there is nothing to say.
///
/// `None` is load-bearing: it is what keeps `--append-system-prompt-file` off
/// the command line entirely. Passing the flag with an empty file would spend
/// a flag, and a line of the preview the user reads, to say nothing.
///
/// The conversation directive is written **in the target language**. It reads
/// as a stronger signal than describing it in English, and it makes the file
/// self-explanatory to anyone who opens it.
pub fn render_instructions(prefs: &AgentPreferences) -> Option<String> {
    if prefs.is_empty() {
        return None;
    }
    let mut out = String::from("# Hangar — preferencias del usuario\n\n");
    if let Some(language) = &prefs.conversation_language {
        out.push_str(&format!(
            "## Idioma de la conversación\n\
             Responde siempre en {language}, sin que haya que pedirlo. Aplica a \
             explicaciones, resúmenes, preguntas y mensajes de error que escribas tú.\n\n"
        ));
    }
    if let Some(language) = &prefs.comment_language {
        out.push_str(&format!(
            "## Idioma del código\n\
             Escribe en {language} los comentarios, los nombres de identificadores, \
             los mensajes de commit y los títulos de pull request. Esto es \
             independiente del idioma de la conversación: puedes conversar en un \
             idioma y escribir el código en otro.\n\n"
        ));
    }
    // An existing convention in the repo beats this file. Without saying so,
    // "comments in Spanish" would license rewriting an English codebase's
    // comments on the way past.
    out.push_str(
        "> Estas preferencias no autorizan a reescribir lo que ya existe: si un \
         fichero sigue otra convención, respétala y aplica esto solo a lo nuevo.\n",
    );
    Some(out)
}

/// The same preferences flattened onto **one line**, for a CLI that takes the
/// text as a config value instead of a file path (Codex).
///
/// The single line is not cosmetic. This value ends up inside the shell string
/// `terminals::account_command_line_full` builds, and a newline there opens
/// zsh's PS2 continuation prompt — the constraint already documented on
/// `account_command_line`. A test asserts the absence of newlines because the
/// failure mode is a terminal that opens into a broken command.
pub fn render_instructions_inline(prefs: &AgentPreferences) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(language) = &prefs.conversation_language {
        parts.push(format!("Responde siempre en {language}, sin que haya que pedirlo."));
    }
    if let Some(language) = &prefs.comment_language {
        parts.push(format!(
            "Escribe en {language} los comentarios, los nombres de identificadores y los mensajes de commit."
        ));
    }
    if parts.is_empty() {
        return None;
    }
    parts.push("Respeta la convención que ya siga cada fichero; esto aplica solo a lo nuevo.".to_string());
    Some(parts.join(" "))
}

/// Materialises the instructions for `project_id` under Hangar's own app data
/// and hands back the path, or `None` when there is nothing to say.
///
/// **In Hangar's data directory, never in the user's repo.** A file written
/// into a worktree would show up in `git status`, and on the wrong day in
/// someone's pull request.
///
/// Rendered at spawn rather than when the setting is saved: a file written on
/// save has to be invalidated when a project is renamed, when the global
/// default moves under a project that inherits it, or when the row is deleted.
/// Writing it as the terminal opens makes every one of those questions
/// disappear — the file cannot be older than the process reading it.
pub fn instruction_file(root: &std::path::Path, conn: &rusqlite::Connection, project_id: Option<&str>) -> Result<Option<ResolvedInstructions>, String> {
    let prefs = resolve(conn, project_id)?;
    // The architecture brief rides this channel because it is the one already
    // proven to reach an agent (`--append-system-prompt-file`, verified with a
    // canary). `memory_files` would have been the tidier home, but nothing has
    // ever fed that table to a turn — which is why its own screen is hidden.
    // A brief written where no agent reads it is a phase that ships broken.
    let brief = project_id.and_then(|id| graph_brief(root, conn, id));
    let diagram = project_id.and_then(|id| diagram_note(conn, id, brief.is_some()));

    // Sections, in the order an agent should meet them: how to talk, what the
    // code looks like, and only then what to do with a tool.
    let sections: Vec<String> = [render_instructions(&prefs), brief.clone(), diagram].into_iter().flatten().collect();
    if sections.is_empty() {
        return Ok(None);
    }
    let text = sections.join("\n\n---\n\n");
    let dir = root.join("agent-instructions");
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    // One file per project (ids are UUIDs, so they are already safe as file
    // names) so two terminals on different projects can't race each other.
    let file = dir.join(format!("{}.md", project_id.unwrap_or("global")));
    std::fs::write(&file, text).map_err(|e| format!("could not write {}: {e}", file.display()))?;
    Ok(Some(ResolvedInstructions {
        file,
        inline: render_instructions_inline(&prefs).unwrap_or_default(),
    }))
}

/// This project's architecture brief, if its graph has been built.
///
/// Reads a small pre-rendered file rather than the graph itself: the graph is
/// megabytes, and this runs on every terminal open. It cannot go stale in a way
/// that matters either — it is written by the same build that writes the graph.
///
/// Silent on every failure. No graphify, no graph, no group: the terminal opens
/// exactly as it does today.
fn graph_brief(root: &std::path::Path, conn: &rusqlite::Connection, project_id: &str) -> Option<String> {
    let (kind, scope_id) = crate::commands::graph::effective_scope(conn, project_id)?;
    let brief = crate::graph::brief_path(&crate::graph::scope_dir(root, kind, &scope_id));
    std::fs::read_to_string(brief).ok().filter(|s| !s.trim().is_empty())
}

/// The note that pairs the diagram skill with the code graph, when a project
/// has both.
///
/// Both already arrive at the agent on their own: the graph as an MCP server
/// (`--mcp-config`), archify as a linked skill folder (`--add-dir`). What was
/// missing is that nothing told the agent they belong together, so a diagram
/// came out guessed from file names while the AST sat one tool call away.
///
/// **Deliberately a note and not a converter.** archify's own SKILL.md spends
/// paragraphs on "at most 12 primary nodes", "one clear main path", "sparse
/// labels" — its whole design assumes an agent composing the spec with
/// judgement. Feeding it a graph of thousands of nodes mechanically would
/// produce exactly the unreadable diagram those rules exist to prevent.
///
/// Composed here rather than written into the brief because the brief is
/// generated when the graph is *built*, and skill selection changes after
/// that. This runs at every terminal open, so it cannot claim a skill that has
/// since been unticked.
fn diagram_note(conn: &rusqlite::Connection, project_id: &str, has_graph: bool) -> Option<String> {
    if !has_graph {
        return None;
    }
    let selected = crate::commands::skills::selected_for_project(conn, project_id).ok()?;
    if !selected.iter().any(|s| s == crate::commands::skills::ARCHIFY) {
        return None;
    }
    Some(
        "# Diagrams of this system\n\n         The `archify` skill and this repo's code graph are both available to you. When you draw          this system, resolve its parts with the graph's MCP tools (`query_graph`, `shortest_path`,          `god_nodes`) instead of inferring them from file names, so every node in the diagram          stands on a relation that is actually in the code.\n\n         Keep archify's own limits: a handful of primary nodes and one clear main path beat a          complete map nobody can read. The graph is there to make the few nodes you do draw the          right ones, not to be drawn in full.\n"
            .to_string(),
    )
}

/// The same instructions in the two shapes the CLIs want: a file for the ones
/// that take a path, one line for the ones that take the text.
#[derive(Debug, Clone)]
pub struct ResolvedInstructions {
    pub file: std::path::PathBuf,
    pub inline: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(conversation: Option<&str>, comment: Option<&str>) -> AgentPreferences {
        AgentPreferences {
            conversation_language: conversation.map(str::to_string),
            comment_language: comment.map(str::to_string),
        }
    }

    fn conn_with_skills(project: &str, names: &[&str]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE project_skills (project_id TEXT NOT NULL, skill_name TEXT NOT NULL, PRIMARY KEY (project_id, skill_name));",
        )
        .unwrap();
        for name in names {
            conn.execute("INSERT INTO project_skills VALUES (?1, ?2)", rusqlite::params![project, name]).unwrap();
        }
        conn
    }

    /// La nota solo tiene sentido cuando existen las dos mitades. Con una sola,
    /// le estaría hablando al agente de una herramienta que no tiene — que es
    /// peor que callarse, porque la va a intentar usar.
    #[test]
    fn the_diagram_note_needs_both_halves() {
        let both = conn_with_skills("p1", &["archify", "otra"]);
        assert!(diagram_note(&both, "p1", true).is_some(), "grafo y skill: la nota va");
        assert!(diagram_note(&both, "p1", false).is_none(), "sin grafo no hay nada que consultar");

        let neither = conn_with_skills("p1", &["otra"]);
        assert!(diagram_note(&neither, "p1", true).is_none(), "sin la skill no hay diagrama que orientar");

        // Y desmarcarla la retira, porque esto se compone al abrir la terminal
        // y no al construir el grafo.
        let unticked = conn_with_skills("p1", &[]);
        assert!(diagram_note(&unticked, "p1", true).is_none(), "desmarcada es desmarcada");
    }

    /// Nombra las herramientas reales del MCP del grafo: una nota que dijera
    /// «usa el grafo» sin decir cómo es un deseo, no una instrucción.
    #[test]
    fn the_diagram_note_names_the_tools_it_expects_to_be_used() {
        let note = diagram_note(&conn_with_skills("p1", &["archify"]), "p1", true).unwrap();
        for tool in ["query_graph", "shortest_path", "god_nodes"] {
            assert!(note.contains(tool), "falta {tool}: {note}");
        }
    }

    #[test]
    fn nothing_configured_renders_no_instructions_at_all() {
        // The whole point: no preferences must mean no flag, not an empty file.
        assert_eq!(render_instructions(&AgentPreferences::default()), None);
    }

    #[test]
    fn a_language_reaches_the_rendered_text() {
        let rendered = render_instructions(&prefs(Some("español"), None)).expect("something to say");
        assert!(rendered.contains("español"), "the language has to appear: {rendered}");
        assert!(!rendered.contains("Idioma del código"), "an unset preference must not render a section");
    }

    #[test]
    fn a_project_value_wins_over_the_global_one() {
        let resolved = prefs(Some("english"), None).over(&prefs(Some("español"), Some("español")));
        assert_eq!(resolved.conversation_language.as_deref(), Some("english"));
    }

    #[test]
    fn an_unset_project_field_still_inherits_the_global_one() {
        // The bug this guards: overriding one field dropping the other.
        let resolved = prefs(Some("english"), None).over(&prefs(Some("español"), Some("português")));
        assert_eq!(resolved.comment_language.as_deref(), Some("português"));
    }

    #[test]
    fn a_blank_choice_is_not_a_language() {
        assert_eq!(clean(Some("   ".to_string())), None);
        assert_eq!(clean(Some("".to_string())), None);
    }

    /// The regression that would break Codex terminals outright: this text is
    /// interpolated into a shell string, and a newline there opens zsh's PS2
    /// continuation prompt instead of running the command.
    #[test]
    fn the_inline_form_never_contains_a_newline() {
        let rendered = render_instructions_inline(&prefs(Some("español"), Some("English"))).expect("something to say");
        assert!(!rendered.contains('\n'), "una sola línea o la terminal se rompe: {rendered:?}");
        assert!(rendered.contains("español") && rendered.contains("English"));
    }

    #[test]
    fn the_inline_form_is_absent_when_nothing_is_configured() {
        // Same contract as the file: nothing to say means no config key at all.
        assert_eq!(render_instructions_inline(&AgentPreferences::default()), None);
    }

    #[test]
    fn the_two_languages_stay_independent() {
        let rendered = render_instructions(&prefs(Some("español"), Some("english"))).expect("something to say");
        assert!(rendered.contains("Responde siempre en español"));
        assert!(rendered.contains("Escribe en english"));
    }
}
