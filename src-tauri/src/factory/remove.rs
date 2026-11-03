//! Taking a project out of the Factory: its line stopped, its orders, runs,
//! team, brain and inbox gone, its worktrees removed — and, only if asked,
//! its folder moved to the Trash and the project out of Hangar too. Never a
//! plain delete of a folder: the Trash can give it back.

use std::path::{Path, PathBuf};

use rusqlite::params;
use tauri::{AppHandle, Runtime};

use crate::db::DbConnection;

/// What was done, for the toast.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Removed {
    pub worktrees: usize,
    pub folder_trashed: bool,
}

/// A folder that can go to the Trash on a «Sí»: a real directory, deep enough,
/// and neither the home folder nor anything above it.
pub fn safe_to_trash(path: &Path, home: Option<&Path>) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("{} no es una carpeta", path.display()));
    }
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    if path.components().count() < 4 {
        return Err(format!("{} está demasiado arriba en el disco para borrarla desde aquí", path.display()));
    }
    if let Some(home) = home.and_then(|h| h.canonicalize().ok()) {
        if home.starts_with(&path) {
            return Err(format!("{} es tu carpeta de usuario o la contiene", path.display()));
        }
    }
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> Result<(), String> {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// The work, once the line is stopped. `root` is the app data folder, where
/// the brain's versions live; `trash` moves a folder to the Trash (injected,
/// so tests don't fill the real one).
pub fn remove(db: &DbConnection, root: Option<&Path>, project_id: &str, delete_folder: bool, trash: &dyn Fn(&Path) -> Result<(), String>) -> Result<Removed, String> {
    // What's needed, with the lock; the disk, without it.
    let (path, worktrees) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let project = super::store::get(&conn, project_id)?;
        let mut stmt = conn.prepare("SELECT DISTINCT worktree FROM factory_runs WHERE project_id = ?1").map_err(|e| e.to_string())?;
        let worktrees: Vec<String> = stmt.query_map(params![project_id], |r| r.get(0)).map_err(|e| e.to_string())?.filter_map(Result::ok).collect();
        (PathBuf::from(project.project_path), worktrees)
    };
    if delete_folder {
        // Refused before anything is touched.
        let home = crate::app_data::home_dir().map(PathBuf::from);
        safe_to_trash(&path, home.as_deref())?;
    }

    let mut removed = 0;
    if !delete_folder {
        // The folder stays: only the Factory's worktrees go. Their branches
        // stay too — they're the repo's history.
        for wt in worktrees.iter().map(PathBuf::from).filter(|w| w.exists()) {
            if git(&path, &["worktree", "remove", "--force", &wt.to_string_lossy()]).is_ok() {
                removed += 1;
            }
        }
        let _ = git(&path, &["worktree", "prune"]);
    }
    if let Some(root) = root {
        let dir = root.join("factory").join(project_id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| format!("no se pudo borrar {}: {e}", dir.display()))?;
        }
    }

    {
        let mut conn = db.0.lock().map_err(|e| e.to_string())?;
        let tx = conn.transaction().map_err(|e| e.to_string())?;
        for table in ["factory_brain_draft", "factory_check_state", "factory_brain_rules", "factory_brain_versions", "factory_amendments", "factory_runs", "factory_orders", "factory_agents", "factory_projects"] {
            tx.execute(&format!("DELETE FROM {table} WHERE project_id = ?1"), params![project_id]).map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
    }

    if delete_folder {
        trash(&path).map_err(|e| format!("salió de la Factory, pero no se pudo mover {} a la Papelera: {e}", path.display()))?;
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        crate::commands::projects::delete_project_rows(&conn, project_id)?;
    }
    Ok(Removed { worktrees: removed, folder_trashed: delete_folder })
}

/// «Eliminar»: ⏹ first — the agent, the gate, pnpm, Playwright — then
/// `remove`.
#[tauri::command]
pub async fn factory_remove_project<R: Runtime>(app: AppHandle<R>, project_id: String, delete_folder: bool) -> Result<Removed, String> {
    super::looping::stop_for_removal(&app, &project_id).await?;
    crate::blocking::run_blocking(move || {
        use tauri::Manager;
        let root = crate::app_data::root(&app).ok();
        let db = app.state::<DbConnection>();
        remove(&db, root.as_deref(), &project_id, delete_folder, &|p| trash::delete(p).map_err(|e| e.to_string()))
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory::line::{self, tests::*};

    /// Hangar's own tables the project's removal touches, beside the Factory's.
    fn with_sessions(w: &World) {
        w.db.0.lock().unwrap().execute_batch("CREATE TABLE IF NOT EXISTS sessions (id TEXT, project_id TEXT, worktree_path TEXT, worktree_branch TEXT, worktree_kind TEXT);").unwrap();
    }

    fn count(w: &World, sql: &str) -> i64 {
        w.db.0.lock().unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    fn started(w: &World) -> line::Run {
        let s = Script::new();
        let c = ctx(w, &s);
        line::start(&c, "p1", &w.order_id).unwrap()
    }

    #[test]
    fn without_the_folder_only_the_factory_goes() {
        let w = world("autonomous", "local", "true");
        with_sessions(&w);
        let run = started(&w);
        assert!(Path::new(&run.worktree).exists());
        let root = w.repo.with_extension("appdata");
        assert!(root.join("factory").join("p1").exists(), "the sealed brain lives there");

        let r = remove(&w.db, Some(&root), "p1", false, &|_| panic!("nothing goes to the Trash on «No»")).unwrap();
        assert_eq!(r, Removed { worktrees: 1, folder_trashed: false });
        for table in ["factory_projects", "factory_agents", "factory_orders", "factory_runs", "factory_steps", "factory_brain_versions", "factory_brain_rules", "factory_amendments"] {
            let n = count(&w, &format!("SELECT COUNT(*) FROM {table}"));
            assert_eq!(n, 0, "{table} still has rows");
        }
        assert!(!Path::new(&run.worktree).exists(), "the worktree is gone");
        assert!(!root.join("factory").join("p1").exists(), "the brain's files are gone");
        assert!(w.repo.exists(), "the folder stays");
        assert_eq!(count(&w, "SELECT COUNT(*) FROM projects WHERE id = 'p1'"), 1, "still a Hangar project");
        assert!(git(&w.repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{}", run.branch)]).is_ok(), "the branch is history: it stays");
    }

    #[test]
    fn with_the_folder_it_goes_to_the_trash_and_out_of_hangar() {
        let w = world("autonomous", "local", "true");
        with_sessions(&w);
        started(&w);
        let bin = std::env::temp_dir().join(format!("hangar-trash-{}", uuid::Uuid::new_v4().simple()));
        let trashed = std::sync::Mutex::new(None);
        let r = remove(&w.db, None, "p1", true, &|p| {
            std::fs::rename(p, &bin).map_err(|e| e.to_string())?;
            *trashed.lock().unwrap() = Some(p.to_path_buf());
            Ok(())
        })
        .unwrap();
        assert!(r.folder_trashed);
        assert_eq!(trashed.into_inner().unwrap().as_deref(), Some(w.repo.as_path()));
        assert!(!w.repo.exists() && bin.exists(), "moved, not deleted");
        assert_eq!(count(&w, "SELECT COUNT(*) FROM projects WHERE id = 'p1'"), 0, "out of Hangar too");
        let _ = std::fs::remove_dir_all(&bin);
    }

    #[test]
    fn the_trash_refuses_what_it_shouldnt_take() {
        let home = std::env::temp_dir().join(format!("hangar-home-{}", uuid::Uuid::new_v4().simple()));
        let project = home.join("Proyects").join("x");
        std::fs::create_dir_all(&project).unwrap();
        assert!(safe_to_trash(Path::new("/"), Some(&home)).is_err());
        assert!(safe_to_trash(&home, Some(&home)).unwrap_err().contains("usuario"));
        assert!(safe_to_trash(&home.join("nope"), Some(&home)).unwrap_err().contains("no es una carpeta"));
        assert!(safe_to_trash(&project, Some(&home)).is_ok());
        let _ = std::fs::remove_dir_all(&home);
    }
}
