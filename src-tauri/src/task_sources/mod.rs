//! Where the rows in the `☰ Tasks` panel come from.
//!
//! The panel already knows how to open N worktrees, each on its task's real
//! branch, with its own account and prompt. This module is the other half: the
//! bit that goes and asks your board what you're supposed to be working on.
//!
//! **The seam is the normalised [`Task`]**, and it is deliberately small — id,
//! title, body, url, an optional branch hint and a status. Sprints, points,
//! custom fields and relations stay out on purpose: the moment one tool's
//! concept enters the model, every other adapter has to fake it.

pub mod clickup;
pub mod github;

use crate::db::DbConnection;
use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;
use uuid::Uuid;

/// One task, as the panel sees it — whatever tool it came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// The tool's own id. Stable between passes, which is what lets a row be
    /// recognised as "the one already open" rather than opened twice.
    pub external_id: String,
    /// The short human key that goes in a branch name: `123` for an issue,
    /// `DEV-42` or a ClickUp custom id when there is one.
    pub key: String,
    pub title: String,
    /// The body. Becomes the brief handed to the agent — see
    /// [`write_task_brief`] for why it doesn't become the prompt directly.
    pub description: String,
    pub url: String,
    pub status: Option<String>,
    /// The branch the source itself suggests, when it has an opinion. ClickUp
    /// does; GitHub doesn't.
    pub branch_hint: Option<String>,
    pub source_id: String,
    pub source_kind: String,
    /// Where the task lives — a ClickUp list, a GitHub repo. Shown in the board
    /// because "Tests" as a title means something different under *Viewer* than
    /// under *Orquestrador*, and both can be assigned to you at once.
    pub container: Option<String>,
    /// What the branch would be called, filled from the link's template once
    /// the task is fetched. Computed here rather than in the panel so the
    /// templating and git's naming rules live in one place — the frontend
    /// prefills the row with it and the user can still edit it.
    #[serde(default)]
    pub suggested_branch: String,
}

/// A configured source. Note what's absent: the token, which lives in the
/// keychain and never travels to the frontend — same treatment as provider
/// accounts and git credentials.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSourceView {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub has_credential: bool,
    pub config: serde_json::Value,
}

/// How a project is wired to a source.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSourceLink {
    pub source_id: String,
    pub kind: String,
    pub label: String,
    /// Opaque to Hangar on purpose: a ClickUp list id, an `owner/repo`, a Jira
    /// board. Every tool's hierarchy is different and none of them is modelled
    /// here — the adapter resolves its own.
    pub scope_id: Option<String>,
    pub branch_template: Option<String>,
}

/// Each source's default branch shape, used when a link has no template.
///
/// GitHub issues have no convention at all, so `issue-123-short-title` is ours.
/// ClickUp does have one, but it's `CU-<id>_<title>` with an underscore, which
/// reads badly next to every other branch in a repo — so the hint fills `{key}`
/// and the shape stays consistent.
pub fn default_branch_template(kind: &str) -> &'static str {
    match kind {
        "github" => "issue-{key}-{slug}",
        _ => "{key}-{slug}",
    }
}

/// Fills a branch template from a task.
///
/// Placeholders: `{key}`, `{id}`, `{slug}`, `{title}`. Anything the template
/// doesn't mention is simply not used — a template of `{key}` alone is a
/// perfectly good answer for a team whose branches are just ticket numbers.
pub fn branch_name(template: &str, task: &Task) -> String {
    let filled = template
        .replace("{key}", &task.key)
        .replace("{id}", &task.external_id)
        .replace("{slug}", &slug(&task.title))
        .replace("{title}", &task.title);
    sanitize_ref(&filled)
}

/// A title reduced to something that reads well in `git branch`.
fn slug(title: &str) -> String {
    let mut out = String::new();
    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            out.extend(ch.to_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
        // 40 characters is enough to recognise a branch and short enough that
        // `git branch` stays readable; the id carries the identity anyway.
        if out.trim_end_matches('-').chars().count() >= 40 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// Makes a filled template safe to hand to `git branch`.
///
/// Git's rules (`git check-ref-format`) reject far more than people expect: a
/// leading or trailing dot, `..`, `@{`, a trailing `.lock`, spaces, `~^:?*[\`,
/// and a trailing slash. A task title contains at least one of those often
/// enough that leaving it to git means a failure at worktree-creation time,
/// several steps after the mistake was made and with a cryptic message.
fn sanitize_ref(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        match ch {
            c if c.is_control() => {}
            ' ' | '~' | '^' | ':' | '?' | '*' | '[' | ']' | '\\' | '\'' | '"' => out.push('-'),
            '.' if out.ends_with('.') => out.push('-'),
            '/' if out.is_empty() || out.ends_with('/') => {}
            c => out.push(c),
        }
    }
    let cleaned = out.trim_matches(|c| c == '-' || c == '.' || c == '/').to_string();
    let cleaned = cleaned.replace("@{", "@-");
    let cleaned = cleaned.strip_suffix(".lock").map(|s| s.to_string()).unwrap_or(cleaned);
    if cleaned.is_empty() {
        "task".to_string()
    } else {
        cleaned
    }
}

/// Writes a task's body into its worktree and returns the **one-line** prompt
/// that points the agent at it.
///
/// Not a nicety. `account_command_line` hands the prompt to the CLI as a
/// positional argument typed into a shell, so it has to be a single line — a
/// body with newlines would trip zsh's continuation prompt inside the quotes
/// and arrive mangled. A task body is markdown with newlines by definition, so
/// it goes to a file and the prompt references it.
///
/// The file lands **inside** the worktree rather than in Hangar's data
/// directory because agents ask for permission before reading outside their
/// working directory, and a permission prompt at the start of every task is
/// exactly the kind of friction this feature exists to remove.
pub fn write_task_brief(worktree: &Path, task: &Task) -> String {
    let dir = worktree.join(".hangar");
    let file = dir.join(format!("task-{}.md", sanitize_ref(&task.key)));
    let body = format!(
        "# {}\n\n{}\n\n---\nSource: {} ({})\n{}\n",
        task.title,
        task.description.trim(),
        task.source_kind,
        task.key,
        task.url,
    );

    if std::fs::create_dir_all(&dir).is_ok() && std::fs::write(&file, body).is_ok() {
        exclude_hangar_dir_locally(worktree);
        return format!("Read .hangar/task-{}.md — it's the task I'm picking up. Start by telling me your plan.", sanitize_ref(&task.key));
    }

    // Couldn't write it: fall back to a one-line prompt built from the title.
    // A worse prompt beats a terminal that doesn't open.
    format!("Work on this task: {} ({})", task.title.replace('\n', " "), task.url)
}

/// Ignores `.hangar/` through `info/exclude` rather than `.gitignore`.
///
/// `.gitignore` is a tracked file, so a rule added on one branch doesn't exist
/// on the others — and a task worktree is by definition a different branch.
/// `info/exclude` lives in the common git directory, applies to every worktree
/// of the clone, and is never committed, so a brief can't be accidentally
/// staged into someone's PR.
fn exclude_hangar_dir_locally(worktree: &Path) {
    let Ok(common) = crate::commands::pull_requests::git_capture(worktree, &["rev-parse", "--git-common-dir"]) else { return };
    let common = if Path::new(&common).is_absolute() { Path::new(&common).to_path_buf() } else { worktree.join(common) };
    let exclude = common.join("info").join("exclude");
    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == ".hangar/" || l.trim() == ".hangar") {
        return;
    }
    let _ = std::fs::create_dir_all(common.join("info"));
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str("# Hangar task briefs and worktrees\n.hangar/\n");
    let _ = std::fs::write(&exclude, updated);
}

// ---------------------------------------------------------------- persistence

fn view_from_row(id: String, kind: String, label: String, credential_ref: Option<String>, config_json: String) -> TaskSourceView {
    TaskSourceView {
        id,
        kind,
        label,
        // Asked of the keychain, not of the row: they come apart, and a row
        // that says "token stored" while the keychain has nothing sends you
        // looking anywhere but at the token.
        has_credential: credential_ref.as_deref().is_some_and(crate::credential_vault::has),
        config: serde_json::from_str(&config_json).unwrap_or(serde_json::Value::Object(Default::default())),
    }
}

#[tauri::command]
pub fn list_task_sources(db: tauri::State<DbConnection>) -> Result<Vec<TaskSourceView>, String> {
    all_sources(&db)
}

fn all_sources(db: &DbConnection) -> Result<Vec<TaskSourceView>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare("SELECT id, kind, label, credential_ref, config_json FROM task_sources ORDER BY created_at")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| Ok(view_from_row(row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)))
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// Adds or updates a source.
///
/// The token is verified against the tool **before** anything is stored: a
/// wrong token doesn't fail loudly at fetch time, it comes back as an empty
/// list, which looks exactly like "you have no tasks" and sends you looking in
/// the wrong place.
#[tauri::command]
pub async fn save_task_source<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: Option<String>,
    kind: String,
    label: String,
    token: Option<String>,
) -> Result<TaskSourceView, String> {
    crate::blocking::run_blocking(move || {
        use tauri::Manager;
        let db = app.state::<DbConnection>();

        let existing_ref: Option<String> = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            match &id {
                Some(id) => conn
                    .query_row("SELECT credential_ref FROM task_sources WHERE id = ?1", params![id], |row| row.get(0))
                    .optional()
                    .map_err(|e| e.to_string())?
                    .flatten(),
                None => None,
            }
        };

        let (credential_ref, config) = match kind.as_str() {
            "github" => {
                // Borrows the `gh` CLI's login rather than holding a credential
                // of its own — one less secret to store, and it's already there.
                github::check_available()?;
                (None, serde_json::json!({}))
            }
            "clickup" => {
                let token = match token.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                    Some(token) => token.to_string(),
                    // Empty means "keep what's stored", the same rule the git
                    // identity form uses — the token never comes back to the
                    // form, so blank is the normal state of an edit.
                    None => match &existing_ref {
                        Some(reference) => crate::credential_vault::get(reference)?,
                        None => return Err("ClickUp needs a personal API token — ClickUp → Settings → Apps → API Token".to_string()),
                    },
                };
                let whoami = clickup::verify(&token)?;
                let reference = existing_ref.clone().unwrap_or_else(|| format!("task-source-{}", Uuid::new_v4()));
                crate::credential_vault::store(&reference, &token)?;
                (Some(reference), serde_json::json!({ "userId": whoami.user_id, "teamIds": whoami.team_ids, "login": whoami.username }))
            }
            other => return Err(format!("unknown task source: {other}")),
        };

        let id = id.unwrap_or_else(|| Uuid::new_v4().to_string());
        let config_json = config.to_string();
        {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO task_sources (id, kind, label, credential_ref, config_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET label = ?3, credential_ref = ?4, config_json = ?5",
                params![id, kind, label, credential_ref, config_json, Utc::now().to_rfc3339()],
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(view_from_row(id, kind, label, credential_ref, config_json))
    })
    .await?
}

#[tauri::command]
pub fn delete_task_source(db: tauri::State<DbConnection>, source_id: String) -> Result<(), String> {
    let credential_ref: Option<String> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let reference = conn
            .query_row("SELECT credential_ref FROM task_sources WHERE id = ?1", params![source_id], |row| row.get(0))
            .optional()
            .map_err(|e| e.to_string())?
            .flatten();
        conn.execute("DELETE FROM task_sources WHERE id = ?1", params![source_id]).map_err(|e| e.to_string())?;
        reference
    };
    // After the row is gone, and ignoring the result: a keychain entry that
    // outlives its row is inert, but a row pointing at a deleted secret is a
    // source that fails on every fetch.
    if let Some(reference) = credential_ref {
        let _ = crate::credential_vault::delete(&reference);
    }
    Ok(())
}

/// What a project can be pointed at inside a source, so the settings screen
/// offers a menu instead of asking for an id copied out of a URL.
///
/// GitHub returns nothing on purpose: its scope is a repo, and the project's
/// own remote is right nearly always — a menu of every repo you can see would
/// be a worse answer than the default.
#[tauri::command]
pub async fn list_source_scopes<R: tauri::Runtime>(app: tauri::AppHandle<R>, source_id: String) -> Result<Vec<clickup::Scope>, String> {
    crate::blocking::run_blocking(move || {
        use tauri::Manager;
        let db = app.state::<DbConnection>();
        let kind: String = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT kind FROM task_sources WHERE id = ?1", params![source_id], |row| row.get(0)).map_err(|e| e.to_string())?
        };
        match kind.as_str() {
            "clickup" => {
                let token = credential_for(&db, &source_id)?;
                let config = config_for(&db, &source_id)?;
                clickup::list_scopes(&token, &config)
            }
            _ => Ok(Vec::new()),
        }
    })
    .await?
}

#[tauri::command]
pub fn link_task_source(
    db: tauri::State<DbConnection>,
    project_id: String,
    source_id: String,
    scope_id: Option<String>,
    branch_template: Option<String>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute(
        "INSERT INTO task_source_links (project_id, source_id, scope_id, branch_template)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(project_id, source_id) DO UPDATE SET scope_id = ?3, branch_template = ?4",
        params![project_id, source_id, scope_id, branch_template],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn unlink_task_source(db: tauri::State<DbConnection>, project_id: String, source_id: String) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    conn.execute("DELETE FROM task_source_links WHERE project_id = ?1 AND source_id = ?2", params![project_id, source_id])
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn list_task_source_links(db: tauri::State<DbConnection>, project_id: String) -> Result<Vec<TaskSourceLink>, String> {
    links_for_project(&db, &project_id)
}

fn links_for_project(db: &DbConnection, project_id: &str) -> Result<Vec<TaskSourceLink>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let mut stmt = conn
        .prepare(
            "SELECT l.source_id, s.kind, s.label, l.scope_id, l.branch_template
             FROM task_source_links l JOIN task_sources s ON s.id = l.source_id
             WHERE l.project_id = ?1 ORDER BY s.created_at",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![project_id], |row| {
            Ok(TaskSourceLink { source_id: row.get(0)?, kind: row.get(1)?, label: row.get(2)?, scope_id: row.get(3)?, branch_template: row.get(4)? })
        })
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    Ok(rows)
}

/// Everything assigned to you, across the sources this project is wired to.
///
/// One source failing doesn't sink the panel: its error is reported alongside
/// whatever the others returned, because a ClickUp token that expired
/// shouldn't hide your GitHub issues.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskFetch {
    pub tasks: Vec<Task>,
    pub errors: Vec<String>,
}

/// Everything one account has assigned to you, with no project in the picture.
///
/// The board is chosen by account first — "Technomar's ClickUp", "the other
/// org's GitHub" — because that's the question you can answer without looking:
/// which of your boards am I working from. Which repo the worktree lands in is
/// a later decision, and belongs next to the button that cuts it.
#[tauri::command]
pub async fn list_source_tasks<R: tauri::Runtime>(app: tauri::AppHandle<R>, source_id: String) -> Result<TaskFetch, String> {
    crate::blocking::run_blocking(move || {
        use tauri::Manager;
        let db = app.state::<DbConnection>();
        let (kind, label): (String, String) = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT kind, label FROM task_sources WHERE id = ?1", params![source_id], |row| Ok((row.get(0)?, row.get(1)?)))
                .map_err(|e| format!("source lookup failed: {e}"))?
        };

        // No link, so no narrowing: the whole account, at its default scope.
        // Any project's own scope is applied later, when one is chosen.
        let link = TaskSourceLink { source_id: source_id.clone(), kind: kind.clone(), label: label.clone(), scope_id: None, branch_template: None };

        let fetched = match kind.as_str() {
            "github" => github::fetch_everywhere(&source_id),
            "clickup" => {
                let token = credential_for(&db, &source_id)?;
                let config = config_for(&db, &source_id)?;
                clickup::fetch(&link, &token, &config)
            }
            other => Err(format!("unknown task source: {other}")),
        };

        match fetched {
            Ok(found) => Ok(TaskFetch { tasks: with_branches(found, &link), errors: Vec::new() }),
            Err(e) => Err(format!("{label}: {e}")),
        }
    })
    .await?
}

/// Fills in each task's branch from the link's template. Here rather than in
/// the panel so git's naming rules live in one place.
fn with_branches(tasks: Vec<Task>, link: &TaskSourceLink) -> Vec<Task> {
    let template = link.branch_template.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or_else(|| default_branch_template(&link.kind));
    tasks
        .into_iter()
        .map(|mut task| {
            task.suggested_branch = branch_name(template, &task);
            task
        })
        .collect()
}

#[tauri::command]
pub async fn list_project_tasks<R: tauri::Runtime>(app: tauri::AppHandle<R>, project_id: String) -> Result<TaskFetch, String> {
    crate::blocking::run_blocking(move || {
        use tauri::Manager;
        let db = app.state::<DbConnection>();
        // A link is how a project *narrows* a source — to one list, or to a
        // different branch shape. Not having one shouldn't mean seeing nothing:
        // adding ClickUp in Settings and getting an empty board until you also
        // tick a checkbox is a ritual, not a feature. So an unlinked project
        // reads every configured source at its default scope.
        let mut links = links_for_project(&db, &project_id)?;
        if links.is_empty() {
            links = all_sources(&db)?
                .into_iter()
                .map(|source| TaskSourceLink { source_id: source.id, kind: source.kind, label: source.label, scope_id: None, branch_template: None })
                .collect();
        }
        if links.is_empty() {
            return Err("no task source is configured yet — Settings → tasks".to_string());
        }

        let project_path: String = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row("SELECT path FROM projects WHERE id = ?1", params![project_id], |row| row.get(0))
                .map_err(|e| format!("project lookup failed: {e}"))?
        };

        let mut tasks = Vec::new();
        let mut errors = Vec::new();
        for link in links {
            let fetched = match link.kind.as_str() {
                "github" => github::fetch(&link, Path::new(&project_path)),
                "clickup" => {
                    let token = credential_for(&db, &link.source_id)?;
                    let config = config_for(&db, &link.source_id)?;
                    clickup::fetch(&link, &token, &config)
                }
                other => Err(format!("unknown task source: {other}")),
            };
            match fetched {
                Ok(found) => tasks.extend(with_branches(found, &link)),
                Err(e) => errors.push(format!("{}: {e}", link.label)),
            }
        }

        if tasks.is_empty() && !errors.is_empty() {
            return Err(errors.join("; "));
        }
        Ok(TaskFetch { tasks, errors })
    })
    .await?
}

fn credential_for(db: &DbConnection, source_id: &str) -> Result<String, String> {
    let reference: Option<String> = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row("SELECT credential_ref FROM task_sources WHERE id = ?1", params![source_id], |row| row.get(0))
            .optional()
            .map_err(|e| e.to_string())?
            .flatten()
    };
    let reference = reference.ok_or_else(|| "this source has no stored token — open its settings and add one".to_string())?;
    crate::credential_vault::get(&reference).map_err(|e| {
        // The raw keyring message ("No matching entry found in secure storage")
        // names neither the source nor the fix, and it's the one people see.
        format!("its token isn't readable from the keychain ({e}) — open Settings → tasks and enter it again")
    })
}

fn config_for(db: &DbConnection, source_id: &str) -> Result<serde_json::Value, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let raw: String = conn
        .query_row("SELECT config_json FROM task_sources WHERE id = ?1", params![source_id], |row| row.get(0))
        .map_err(|e| e.to_string())?;
    Ok(serde_json::from_str(&raw).unwrap_or(serde_json::Value::Object(Default::default())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(key: &str, title: &str) -> Task {
        Task {
            external_id: "abc123".to_string(),
            key: key.to_string(),
            title: title.to_string(),
            description: "body".to_string(),
            url: "https://example.test/1".to_string(),
            status: None,
            branch_hint: None,
            source_id: "src1".to_string(),
            source_kind: "clickup".to_string(),
            container: None,
            suggested_branch: String::new(),
        }
    }

    #[test]
    fn a_branch_name_is_built_from_the_template() {
        let filled = branch_name("issue-{key}-{slug}", &task("128", "Fix the login redirect"));
        assert_eq!(filled, "issue-128-fix-the-login-redirect");
    }

    /// The characters git rejects are common in task titles — a slash, a colon
    /// after a component prefix, brackets around a ticket tag. Left to git they
    /// fail at `worktree add`, several steps after the mistake and with a
    /// message that names none of this.
    #[test]
    fn a_title_full_of_characters_git_rejects_still_produces_a_usable_branch() {
        let filled = branch_name("{key}-{slug}", &task("DEV-9", "feat(api): support ~50% [beta] loads?"));
        assert!(!filled.contains(['~', '^', ':', '?', '*', '[', ']', ' ']), "got: {filled}");
        assert!(!filled.starts_with('-') && !filled.ends_with('-'), "got: {filled}");
        assert!(filled.starts_with("DEV-9-"), "the key must survive intact: {filled}");
    }

    /// A title of nothing but punctuation slugs to an empty string, and an
    /// empty branch name is an error at worktree time rather than here.
    #[test]
    fn a_title_that_slugs_to_nothing_still_yields_a_branch() {
        let filled = branch_name("{slug}", &task("42", "!!! ???"));
        assert_eq!(filled, "task");
    }

    /// Long titles are the norm on a board, and a 200-character branch name is
    /// unusable in every tool that shows branches.
    #[test]
    fn a_long_title_is_truncated_but_the_key_is_not() {
        let long = "a".repeat(200);
        let filled = branch_name("{key}-{slug}", &task("CU-86a", &long));
        assert!(filled.len() < 60, "got {} chars: {filled}", filled.len());
        assert!(filled.starts_with("CU-86a-"), "got: {filled}");
    }

    /// The reason this whole indirection exists: the prompt is typed into a
    /// shell as a positional argument, so **whatever happens, it stays one
    /// line**. A task body is markdown with newlines, and a newline in there
    /// would submit half a sentence and leave zsh at a continuation prompt.
    #[test]
    fn a_multiline_body_never_reaches_the_prompt() {
        let dir = std::env::temp_dir().join(format!("hangar-test-brief-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut task = task("DEV-42", "Seed the signal catalog");
        task.description = "First line.\n\n- a bullet\n- another\n\n```rust\nfn main() {}\n```".to_string();

        let prompt = write_task_brief(&dir, &task);
        assert!(!prompt.contains('\n'), "the prompt must stay a single line: {prompt:?}");

        let written = std::fs::read_to_string(dir.join(".hangar").join("task-DEV-42.md")).unwrap();
        assert!(written.contains("- a bullet"), "the body belongs in the file, whole");
        assert!(written.contains("https://example.test/1"), "the brief keeps a link back to the task");
        assert!(prompt.contains("task-DEV-42.md"), "and the prompt has to point at it: {prompt}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A worktree that can't be written to (read-only checkout, full disk)
    /// must still open a terminal — with a worse prompt, which beats none.
    #[test]
    fn an_unwritable_worktree_falls_back_to_a_one_line_prompt() {
        let missing = std::env::temp_dir().join(format!("hangar-test-missing-{}/nested", Uuid::new_v4()));
        let mut task = task("7", "Bump deps");
        task.description = "multi\nline".to_string();
        // A path under a directory that doesn't exist and can't be created is
        // hard to arrange portably; a file where the directory should be is not.
        let file = std::env::temp_dir().join(format!("hangar-test-file-{}", Uuid::new_v4()));
        std::fs::write(&file, "not a directory").unwrap();
        let prompt = write_task_brief(&file, &task);
        assert!(!prompt.contains('\n'), "still one line: {prompt:?}");
        assert!(prompt.contains("Bump deps"), "and still says what the task is: {prompt}");
        let _ = std::fs::remove_file(&file);
        let _ = std::fs::remove_dir_all(&missing);
    }

    /// `..` and `@{` are rejected by git anywhere in a ref, and both turn up in
    /// real titles ("wait.. why", "@{here}").
    #[test]
    fn sequences_git_reserves_are_defused() {
        assert!(!sanitize_ref("wait..why").contains(".."));
        assert!(!sanitize_ref("fix @{here}").contains("@{"));
        assert!(!sanitize_ref("some.lock").ends_with(".lock"));
    }
}
