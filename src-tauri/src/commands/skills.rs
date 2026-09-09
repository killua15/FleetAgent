//! Your skills, and which projects turn them on.
//!
//! A skill is a folder with a `SKILL.md` inside — an open standard, originally
//! Anthropic's and now read by Claude Code, Codex, Gemini CLI and dozens of
//! other clients. Hangar neither invents a format nor converts between them;
//! it stores the folders and points each CLI at the same one.
//!
//! **Verified by running it**, not by reading docs: a canary skill carrying a
//! unique token came back from the agent when its directory was passed with
//! `--add-dir`, and was unknown to the same agent without it. The canary was a
//! *symlink*, which is what makes one library shared by many projects possible
//! without copying anything.
//!
//! Two homes, on purpose:
//! - the library is **on disk** (`<app_data>/skills/<name>/`), so a skill can
//!   carry `scripts/` and `references/` like the standard allows;
//! - the per-project selection is **in SQLite** (`project_skills`), because
//!   that is a relation, not a document.

use crate::db::DbConnection;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;

/// The folder name of the diagram skill, named once because two places need
/// it: the environment that silences its update check, and the note that pairs
/// it with the code graph.
pub const ARCHIFY: &str = "archify";

/// Environment that stops an enabled skill from calling out, for the skills
/// known to do it.
///
/// Not a general policy — Hangar cannot tell from a `SKILL.md` whether the
/// scripts it ships touch the network. It is a short table of the ones checked
/// by hand, and today it has one row: archify's own instructions tell the agent
/// to "run the packaged checker `scripts/check-update.mjs`", which reaches a
/// manifest over the network. Hangar's README promises nothing leaves the
/// machine, and a skill the user installed *through Hangar* is Hangar's to be
/// honest about.
///
/// Rendered into the visible command line rather than set on the process
/// quietly, the same choice `ProviderSpec::extra_env` makes: what the picker
/// previews is what runs, and someone reading it can see the silencing happen.
const SILENCING_ENV: &[(&str, &str, &str)] = &[(ARCHIFY, "ARCHIFY_UPDATE_CHECK_DISABLED", "1")];

/// The skills actually linked into a project's directory.
///
/// Read from the symlinks rather than from the saved selection: those are the
/// skills the CLI will really see — `skills_dir` drops any whose folder went
/// missing — so there is no second source of truth to drift. Everything that
/// keys off "is this skill in play" starts here.
pub fn linked_skills(skills_parent: &Path) -> Vec<String> {
    let dir = skills_parent.join(".claude").join("skills");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string()).collect()
}

/// Pure over the linked names, so the table can be tested without a filesystem.
pub fn silencing_env(linked: &[String]) -> Vec<(String, String)> {
    SILENCING_ENV
        .iter()
        .filter(|(skill, _, _)| linked.iter().any(|p| p == skill))
        .map(|(_, key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// Where a project's delivered diagrams go, handed to the agent so the
/// instruction note can point at it by name.
///
/// Only when the diagram skill is actually linked: a variable naming a
/// directory nothing will ever write to is noise on a command line someone
/// reads. Hangar creates the directory here rather than hoping the agent will
/// — `deliver` writes a file, it does not make a path.
pub fn diagrams_env(linked: &[String], app_data: &Path, project_id: &str) -> Vec<(String, String)> {
    if !linked.iter().any(|s| s == ARCHIFY) {
        return Vec::new();
    }
    let dir = crate::diagrams::dir(app_data, project_id);
    if std::fs::create_dir_all(&dir).is_err() {
        return Vec::new();
    }
    vec![("HANGAR_DIAGRAMS".to_string(), dir.to_string_lossy().to_string())]
}

/// One skill in the user's library.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    /// The folder name, which is also the slash command (`/<name>`).
    pub name: String,
    /// The frontmatter `description`. Load-bearing rather than decorative:
    /// it is the only thing an agent sees until it decides to open the skill,
    /// so it is what determines whether the skill ever triggers.
    pub description: String,
    /// Everything after the frontmatter.
    pub body: String,
    /// Whether the folder holds more than `SKILL.md` — scripts, references,
    /// assets. Shown so the UI can say "this one has files" instead of
    /// pretending a folder is a text box.
    pub has_extra_files: bool,
}

/// Folder names become paths, slash commands and symlink targets. Anything
/// outside this set could escape the library directory (`../`) or produce a
/// command nobody can type.
fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a skill needs a name".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err(format!("'{name}' can only use letters, numbers, hyphens and underscores"));
    }
    Ok(())
}

pub(crate) fn library_dir(root: &Path) -> PathBuf {
    root.join("skills")
}

/// Splits `SKILL.md` into its frontmatter description and its body.
///
/// Deliberately tolerant: a file edited by hand outside Hangar, or one whose
/// frontmatter got mangled, still lists and still opens. Refusing to show a
/// skill because its header is malformed would hide the very thing the user
/// needs to fix.
fn parse_skill_md(raw: &str, name: &str) -> (String, String) {
    let Some(rest) = raw.strip_prefix("---") else {
        return (String::new(), raw.to_string());
    };
    let Some(end) = rest.find("\n---") else {
        return (String::new(), raw.to_string());
    };
    let (frontmatter, body) = rest.split_at(end);
    let description = frontmatter
        .lines()
        .find_map(|line| line.trim().strip_prefix("description:").map(|v| v.trim().to_string()))
        .unwrap_or_default();
    let body = body.trim_start_matches("\n---").trim_start_matches('\n').to_string();
    let _ = name;
    (description, body)
}

/// Every frontmatter key Hangar's editor does **not** own, kept verbatim.
///
/// The standard has more fields than this UI edits — `allowed-tools`,
/// `license`, `compatibility`, and the `metadata` block that `gh skill
/// install` injects to record where a skill came from. Rewriting the header
/// from just name+description silently destroys all of them: `gh skill update`
/// stops being able to update the skill, and a pre-approval the skill needed
/// disappears without a word.
///
/// Line-based on purpose rather than a YAML round-trip: a parser would also
/// have to re-emit, and re-emitting normalises quoting, key order and
/// indentation — churning a file the user may be tracking in git.
fn preserved_frontmatter(raw: &str) -> String {
    let Some(rest) = raw.strip_prefix("---") else { return String::new() };
    let Some(end) = rest.find("\n---") else { return String::new() };
    let mut kept = String::new();
    // `metadata:` owns the indented lines under it, so a dropped key has to
    // drop its continuation lines too — otherwise they reattach to whatever
    // key comes next and change its meaning.
    let mut dropping = false;
    for line in rest[..end].lines() {
        if line.trim().is_empty() {
            continue;
        }
        let is_continuation = line.starts_with(' ') || line.starts_with('\t') || line.trim_start().starts_with('-');
        if is_continuation {
            if !dropping {
                kept.push_str(line);
                kept.push('\n');
            }
            continue;
        }
        dropping = line.starts_with("name:") || line.starts_with("description:");
        if !dropping {
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept
}

fn render_skill_md(skill: &Skill, preserved: &str) -> String {
    // The description is quote-free but newline-stripped: YAML ends a plain
    // scalar at the line break and would silently drop the rest of the
    // sentence — and the description is what decides whether a skill fires.
    let description = skill.description.replace('\n', " ");
    format!(
        "---\nname: {}\ndescription: {}\n{}---\n\n{}\n",
        skill.name,
        description,
        preserved,
        skill.body.trim_end()
    )
}

// ---------------------------------------------------------------- library

fn read_library(root: &Path) -> Result<Vec<Skill>, String> {
    let dir = library_dir(root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| format!("could not read {}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let skill_md = entry.path().join("SKILL.md");
        let Ok(raw) = std::fs::read_to_string(&skill_md) else {
            // A folder with no SKILL.md is not a skill. Skipped rather than
            // errored: one stray directory must not empty the whole list.
            continue;
        };
        let (description, body) = parse_skill_md(&raw, &name);
        let has_extra_files = std::fs::read_dir(entry.path())
            .map(|mut d| d.any(|e| e.map(|e| e.file_name() != "SKILL.md").unwrap_or(false)))
            .unwrap_or(false);
        found.push(Skill { name, description, body, has_extra_files });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

#[tauri::command]
pub fn list_skills<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Result<Vec<Skill>, String> {
    read_library(&crate::app_data::root(&app)?)
}

#[tauri::command]
pub fn save_skill<R: tauri::Runtime>(app: tauri::AppHandle<R>, input: Skill) -> Result<Skill, String> {
    valid_name(&input.name)?;
    let dir = library_dir(&crate::app_data::root(&app)?).join(&input.name);
    let file = dir.join("SKILL.md");
    // Read what is already there and carry forward the header fields this
    // editor doesn't own. Done here rather than by round-tripping through the
    // frontend on purpose: the preservation then holds no matter what the UI
    // sends, so a client that has never heard of `allowed-tools` cannot drop
    // it.
    let preserved = std::fs::read_to_string(&file).map(|raw| preserved_frontmatter(&raw)).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    std::fs::write(&file, render_skill_md(&input, &preserved)).map_err(|e| format!("could not write SKILL.md: {e}"))?;
    Ok(input)
}

#[tauri::command]
pub fn delete_skill<R: tauri::Runtime>(app: tauri::AppHandle<R>, db: State<DbConnection>, name: String) -> Result<(), String> {
    valid_name(&name)?;
    let dir = library_dir(&crate::app_data::root(&app)?).join(&name);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("could not remove {}: {e}", dir.display()))?;
    }
    // Drop the selections too, or every project keeps pointing at a skill that
    // no longer exists and the materialiser has to guess what that means.
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM project_skills WHERE skill_name = ?1", params![name]).map_err(|e| e.to_string())?;
    Ok(())
}

/// Opens a skill's folder in Finder, and returns the path either way.
///
/// This is how `scripts/`, `references/` and `assets/` get added — the parts of
/// the standard that a textarea can't express — without Hangar growing a file
/// browser it would then have to maintain.
///
/// macOS-only, like the rest of Hangar. The path comes back even when opening
/// fails, because a path the user can paste somewhere still solves their
/// problem.
#[tauri::command]
pub async fn reveal_skill_directory<R: tauri::Runtime>(app: tauri::AppHandle<R>, name: String) -> Result<String, String> {
    crate::blocking::run_blocking(move || {
        valid_name(&name)?;
        let dir = library_dir(&crate::app_data::root(&app)?).join(&name);
        if !dir.is_dir() {
            return Err(format!("'{name}' has no folder yet — save it first"));
        }
        let path = dir.to_string_lossy().to_string();
        std::process::Command::new("open").arg(&dir).spawn().map_err(|e| format!("could not open {path}: {e}"))?;
        Ok(path)
    })
    .await?
}

// -------------------------------------------------------------- selection

pub(crate) fn selected_for_project(conn: &rusqlite::Connection, project_id: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT skill_name FROM project_skills WHERE project_id = ?1 ORDER BY skill_name")
        .map_err(|e| e.to_string())?;
    let rows = stmt.query_map(params![project_id], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_project_skills(db: State<DbConnection>, project_id: String) -> Result<Vec<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    selected_for_project(&conn, &project_id)
}

#[tauri::command]
pub fn set_project_skill(db: State<DbConnection>, project_id: String, skill_name: String, enabled: bool) -> Result<Vec<String>, String> {
    valid_name(&skill_name)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if enabled {
        conn.execute(
            "INSERT INTO project_skills (project_id, skill_name) VALUES (?1, ?2) ON CONFLICT DO NOTHING",
            params![project_id, skill_name],
        )
    } else {
        conn.execute("DELETE FROM project_skills WHERE project_id = ?1 AND skill_name = ?2", params![project_id, skill_name])
    }
    .map_err(|e| e.to_string())?;
    selected_for_project(&conn, &project_id)
}

// --------------------------------------------------------- materialisation

/// Builds the directory a CLI gets pointed at, and returns it — or `None`
/// when this project has no skills selected.
///
/// `None` is load-bearing, exactly as it is for the preferences file: it keeps
/// `--add-dir` off the command line entirely instead of pointing it at an
/// empty folder.
///
/// **Symlinks, not copies.** Editing a skill once has to change it everywhere
/// it is used; copying would make each project a stale fork of the library.
/// Verified against the real CLI — a symlinked skill loads.
///
/// **A tightly scoped subdirectory, never the app data root.** `--add-dir`
/// grants file access as well as loading skills, so pointing it at the root
/// would hand the agent Hangar's own database.
///
/// Rebuilt on every spawn, like the preferences file: a directory assembled
/// when the selection was saved would have to be invalidated when a skill is
/// renamed, deleted, or unticked. Assembling it as the terminal opens means
/// none of those questions exist.
pub fn skills_dir(root: &Path, conn: &rusqlite::Connection, project_id: &str) -> Result<Option<PathBuf>, String> {
    let selected = selected_for_project(conn, project_id)?;
    let library = library_dir(root);
    // A selection naming a skill whose folder is gone contributes nothing —
    // better a working terminal with fewer skills than a dangling symlink the
    // CLI trips over.
    let usable: Vec<String> = selected.into_iter().filter(|name| library.join(name).join("SKILL.md").is_file()).collect();

    let base = root.join("projects").join(project_id);
    let dir = base.join(".claude").join("skills");
    if usable.is_empty() {
        // Leaving yesterday's symlinks behind would keep serving skills the
        // user has since turned off.
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(None);
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    // Clear what is no longer selected before linking what is. Unticking a
    // skill has to actually remove it, and the cheapest correct way to say
    // that is to rebuild the directory's contents.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let keep = usable.iter().any(|name| std::ffi::OsStr::new(name) == entry.file_name());
            if !keep {
                let _ = std::fs::remove_file(entry.path());
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
    }

    for name in &usable {
        let link = dir.join(name);
        if link.symlink_metadata().is_ok() {
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(library.join(name), &link).map_err(|e| format!("could not link skill '{name}': {e}"))?;
    }

    Ok(Some(base))
}

// --------------------------------------------------- skills siempre activas

pub(crate) fn always_on(conn: &rusqlite::Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn.prepare("SELECT skill_name FROM always_on_skills ORDER BY skill_name").map_err(|e| e.to_string())?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
    rows.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_always_on_skills(db: State<DbConnection>) -> Result<Vec<String>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    always_on(&conn)
}

#[tauri::command]
pub fn set_always_on_skill(db: State<DbConnection>, skill_name: String, enabled: bool) -> Result<Vec<String>, String> {
    valid_name(&skill_name)?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    if enabled {
        conn.execute("INSERT INTO always_on_skills (skill_name) VALUES (?1) ON CONFLICT DO NOTHING", params![skill_name])
    } else {
        conn.execute("DELETE FROM always_on_skills WHERE skill_name = ?1", params![skill_name])
    }
    .map_err(|e| e.to_string())?;
    always_on(&conn)
}

/// True when `path` is a symlink Hangar itself put there — the only thing this
/// module is allowed to remove from a config directory.
///
/// **This function is the safety rule.** An account's config directory can be
/// one the user already had (`~/.claude-work`, adopted through Settings →
/// General), full of their own files. A real directory, or a link pointing
/// somewhere else, belongs to them and must survive everything Hangar does.
fn is_ours(path: &Path, library: &Path) -> bool {
    let Ok(meta) = path.symlink_metadata() else { return false };
    if !meta.file_type().is_symlink() {
        return false;
    }
    // `read_link` rather than `canonicalize`: a link whose target was deleted
    // still has to be recognised as ours so it can be cleaned up.
    std::fs::read_link(path).map(|target| target.starts_with(library)).unwrap_or(false)
}

/// Links the always-on skills into one account's config directory, and removes
/// the ones Hangar previously put there and are no longer marked.
///
/// **No flag involved.** These land where every CLI already looks for its own
/// skills — `$CLAUDE_CONFIG_DIR/skills`, `$CODEX_HOME/skills` — which is what
/// makes them reach providers whose command-line flags Hangar has not verified.
/// Confirmed for Claude Code by running it: a probe skill in `~/.claude/skills`
/// was invisible while `CLAUDE_CONFIG_DIR` pointed elsewhere, and loaded as soon
/// as it moved into the directory that variable named.
///
/// Safe to call concurrently. The content depends on the *account*, never on the
/// project, so two terminals of the same account on different projects write
/// exactly the same links — and creating a link that already exists is a no-op.
pub fn link_always_on(root: &Path, conn: &rusqlite::Connection, config_dir: &Path) -> Result<(), String> {
    let library = library_dir(root);
    let marked: Vec<String> = always_on(conn)?
        .into_iter()
        .filter(|name| library.join(name).join("SKILL.md").is_file())
        .collect();

    let dir = config_dir.join("skills");
    if marked.is_empty() && !dir.is_dir() {
        // Nothing marked and nothing to clean: don't create a directory in
        // someone's config dir just to leave it empty.
        return Ok(());
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;

    // Retirar solo lo nuestro. Todo lo demás —una skill que instalaste tú, una
    // carpeta de verdad— se queda donde está.
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let keep = marked.iter().any(|name| std::ffi::OsStr::new(name) == entry.file_name());
            if !keep && is_ours(&entry.path(), &library) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    for name in &marked {
        let link = dir.join(name);
        if link.symlink_metadata().is_ok() {
            // Ya hay algo con ese nombre. Si es nuestro, ya apunta bien; si es
            // del usuario, gana él y no se toca.
            continue;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(library.join(name), &link).map_err(|e| format!("could not link skill '{name}': {e}"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-skills-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn conn_with_selection(project: &str, names: &[&str]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE project_skills (project_id TEXT NOT NULL, skill_name TEXT NOT NULL, PRIMARY KEY (project_id, skill_name));")
            .unwrap();
        for name in names {
            conn.execute("INSERT INTO project_skills VALUES (?1, ?2)", params![project, name]).unwrap();
        }
        conn
    }

    fn write_skill(root: &Path, name: &str) {
        let dir = library_dir(root).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: does {name}\n---\n\nbody of {name}\n")).unwrap();
    }

    #[test]
    fn nothing_selected_means_no_directory_at_all() {
        // The whole point: no skills must mean no `--add-dir`, not a flag
        // pointing at an empty folder.
        let root = temp();
        let conn = conn_with_selection("p1", &[]);
        assert_eq!(skills_dir(&root, &conn, "p1").unwrap(), None);
    }

    #[test]
    fn a_selected_skill_becomes_a_symlink_the_cli_can_find() {
        let root = temp();
        write_skill(&root, "canary");
        let conn = conn_with_selection("p1", &["canary"]);

        let base = skills_dir(&root, &conn, "p1").unwrap().expect("a directory");
        let link = base.join(".claude").join("skills").join("canary");
        assert!(link.symlink_metadata().is_ok(), "the skill has to be there");
        assert!(link.join("SKILL.md").is_file(), "and resolve to the real folder");
        // A copy would make each project a stale fork of the library.
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink(), "it must be a link, not a copy");
    }

    #[test]
    fn unticking_a_skill_actually_removes_it() {
        // The regression this guards: a directory that only ever grows, so a
        // skill turned off keeps being served.
        let root = temp();
        write_skill(&root, "one");
        write_skill(&root, "two");

        let both = conn_with_selection("p1", &["one", "two"]);
        skills_dir(&root, &both, "p1").unwrap();

        let only_one = conn_with_selection("p1", &["one"]);
        let base = skills_dir(&root, &only_one, "p1").unwrap().expect("a directory");
        let skills = base.join(".claude").join("skills");
        assert!(skills.join("one").symlink_metadata().is_ok());
        assert!(skills.join("two").symlink_metadata().is_err(), "'two' was turned off and must be gone");
    }

    #[test]
    fn a_selection_pointing_at_a_deleted_skill_is_ignored() {
        let root = temp();
        let conn = conn_with_selection("p1", &["ghost"]);
        assert_eq!(skills_dir(&root, &conn, "p1").unwrap(), None, "a dangling name must not produce a broken link");
    }

    #[test]
    fn the_directory_handed_over_never_exposes_the_app_data_root() {
        // `--add-dir` grants file access as well as loading skills, so this
        // path reaching the app data root would hand over hangar.db.
        let root = temp();
        write_skill(&root, "canary");
        let conn = conn_with_selection("p1", &["canary"]);
        let base = skills_dir(&root, &conn, "p1").unwrap().unwrap();
        assert_ne!(base, root);
        assert!(base.starts_with(root.join("projects")), "got: {}", base.display());
    }

    #[test]
    fn a_skill_file_survives_a_round_trip() {
        let skill = Skill {
            name: "canary".to_string(),
            description: "returns a token".to_string(),
            body: "Reply with CANARY.".to_string(),
            has_extra_files: false,
        };
        let (description, body) = parse_skill_md(&render_skill_md(&skill, ""), "canary");
        assert_eq!(description, "returns a token");
        assert_eq!(body.trim(), "Reply with CANARY.");
    }

    /// Editing an installed skill must not silently destroy its header.
    ///
    /// This is the real frontmatter `gh skill install` writes. Losing the
    /// `metadata` block breaks `gh skill update`; losing `allowed-tools`
    /// removes a pre-approval the skill relies on. Both would be silent.
    #[test]
    fn saving_an_installed_skill_keeps_the_fields_the_editor_does_not_own() {
        let installed = "---\n\
             description: Add educational comments.\n\
             metadata:\n    \
                 github-path: skills/add-educational-comments\n    \
                 github-repo: https://github.com/github/awesome-copilot\n    \
                 github-tree-sha: e9d3c89\n\
             allowed-tools: Read Grep\n\
             license: MIT\n\
             name: add-educational-comments\n\
             ---\n\n\
             # Body\n";

        let preserved = preserved_frontmatter(installed);
        let edited = Skill {
            name: "add-educational-comments".to_string(),
            description: "Edited from Hangar.".to_string(),
            body: "# Body".to_string(),
            has_extra_files: false,
        };
        let rewritten = render_skill_md(&edited, &preserved);

        assert!(rewritten.contains("github-repo: https://github.com/github/awesome-copilot"), "got: {rewritten}");
        assert!(rewritten.contains("github-tree-sha: e9d3c89"), "el sha es lo que permite a `gh skill update` detectar cambios");
        assert!(rewritten.contains("allowed-tools: Read Grep"), "got: {rewritten}");
        assert!(rewritten.contains("license: MIT"), "got: {rewritten}");
        // Y lo que sí edita la UI queda actualizado, una sola vez.
        assert!(rewritten.contains("description: Edited from Hangar."), "got: {rewritten}");
        assert_eq!(rewritten.matches("description:").count(), 1, "sin duplicar la clave: {rewritten}");
        assert_eq!(rewritten.matches("name:").count(), 1, "got: {rewritten}");
    }

    #[test]
    fn a_dropped_key_takes_its_indented_lines_with_it() {
        // Si `description` fuera un bloque multilínea, sus continuaciones no
        // pueden quedarse sueltas: se reengancharían a la clave siguiente y
        // cambiarían su significado.
        let raw = "---\ndescription: |\n  first\n  second\nlicense: MIT\n---\n\nbody\n";
        let preserved = preserved_frontmatter(raw);
        assert!(preserved.contains("license: MIT"));
        assert!(!preserved.contains("first"), "las continuaciones se van con su clave: {preserved:?}");
    }

    #[test]
    fn a_skill_with_a_plain_header_preserves_nothing_extra() {
        let raw = "---\nname: canary\ndescription: does things\n---\n\nbody\n";
        assert_eq!(preserved_frontmatter(raw), "");
    }

    #[test]
    fn a_multiline_description_cannot_truncate_the_frontmatter() {
        // YAML ends a plain scalar at the newline, so an unflattened
        // description would silently drop everything after the first line —
        // and the description is what decides whether a skill ever triggers.
        let skill = Skill {
            name: "canary".to_string(),
            description: "first line\nsecond line".to_string(),
            body: "body".to_string(),
            has_extra_files: false,
        };
        let rendered = render_skill_md(&skill, "");
        let (description, _) = parse_skill_md(&rendered, "canary");
        assert_eq!(description, "first line second line");
    }

    fn conn_with_always_on(names: &[&str]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE always_on_skills (skill_name TEXT PRIMARY KEY);").unwrap();
        for name in names {
            conn.execute("INSERT INTO always_on_skills VALUES (?1)", params![name]).unwrap();
        }
        conn
    }

    #[test]
    fn an_always_on_skill_lands_in_the_account_config_dir() {
        let root = temp();
        write_skill(&root, "canary");
        let config = root.join("cuenta-a");
        std::fs::create_dir_all(&config).unwrap();

        link_always_on(&root, &conn_with_always_on(&["canary"]), &config).unwrap();

        let link = config.join("skills").join("canary");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink(), "enlace, no copia");
        assert!(link.join("SKILL.md").is_file(), "y resuelve a la carpeta real");
    }

    #[test]
    fn unmarking_removes_it_from_the_config_dir() {
        let root = temp();
        write_skill(&root, "canary");
        let config = root.join("cuenta-a");
        std::fs::create_dir_all(&config).unwrap();

        link_always_on(&root, &conn_with_always_on(&["canary"]), &config).unwrap();
        link_always_on(&root, &conn_with_always_on(&[]), &config).unwrap();

        assert!(config.join("skills").join("canary").symlink_metadata().is_err(), "desmarcada tiene que irse");
    }

    /// **La regla de seguridad del módulo.**
    ///
    /// El config dir de una cuenta puede ser uno que el usuario ya tenía
    /// (`~/.claude-work`, adoptado desde Settings → General) con sus propias
    /// skills dentro. Hangar solo puede tocar lo que puso él.
    #[test]
    fn the_users_own_skills_survive_everything_hangar_does() {
        let root = temp();
        write_skill(&root, "canary");
        let config = root.join("cuenta-a");
        let skills = config.join("skills");
        std::fs::create_dir_all(&skills).unwrap();

        // Una skill de verdad, del usuario, escrita a mano.
        let suya = skills.join("mia-propia");
        std::fs::create_dir_all(&suya).unwrap();
        std::fs::write(suya.join("SKILL.md"), "---\nname: mia-propia\n---\n\nno la toques\n").unwrap();

        // Y un enlace suyo a un sitio que no es la biblioteca de Hangar.
        let fuera = root.join("otro-sitio");
        std::fs::create_dir_all(&fuera).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&fuera, skills.join("enlace-ajeno")).unwrap();

        // Ciclo completo: marcar, desmarcar, volver a marcar.
        link_always_on(&root, &conn_with_always_on(&["canary"]), &config).unwrap();
        link_always_on(&root, &conn_with_always_on(&[]), &config).unwrap();
        link_always_on(&root, &conn_with_always_on(&["canary"]), &config).unwrap();

        assert!(suya.join("SKILL.md").is_file(), "una carpeta real del usuario no se borra jamás");
        assert!(skills.join("enlace-ajeno").symlink_metadata().is_ok(), "un enlace que apunta fuera tampoco");
        assert!(skills.join("canary").symlink_metadata().is_ok(), "y la nuestra sí vuelve");
    }

    #[test]
    fn linking_twice_is_a_no_op() {
        // Dos terminales de la misma cuenta pueden arrancar a la vez. Como el
        // contenido no depende del proyecto, ambas escriben lo mismo y la
        // segunda no puede romper nada.
        let root = temp();
        write_skill(&root, "canary");
        let config = root.join("cuenta-a");
        std::fs::create_dir_all(&config).unwrap();

        let conn = conn_with_always_on(&["canary"]);
        link_always_on(&root, &conn, &config).unwrap();
        link_always_on(&root, &conn, &config).unwrap();

        let entries = std::fs::read_dir(config.join("skills")).unwrap().count();
        assert_eq!(entries, 1, "no se duplica");
    }

    #[test]
    fn nothing_marked_does_not_create_a_directory_in_someone_elses_config() {
        let root = temp();
        let config = root.join("cuenta-a");
        std::fs::create_dir_all(&config).unwrap();

        link_always_on(&root, &conn_with_always_on(&[]), &config).unwrap();

        assert!(!config.join("skills").exists(), "sin nada que poner, no se ensucia el directorio ajeno");
    }

    #[test]
    fn a_name_cannot_escape_the_library_directory() {
        assert!(valid_name("../../etc").is_err());
        assert!(valid_name("has/slash").is_err());
        assert!(valid_name("").is_err());
        assert!(valid_name("code-review_2").is_ok());
    }
}
