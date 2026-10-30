//! Step 1 of the wizard on disk: a new folder for a new project, its first
//! commit on `main` with `dev` next to it, and what an existing folder's repo
//! looks like.

use std::path::{Path, PathBuf};

use serde::Serialize;

fn git(cwd: &Path, args: &[&str], env: &[(String, String)]) -> Result<String, String> {
    crate::commands::pull_requests::git_capture_with_env(cwd, args, env)
}

/// What the wizard shows about a folder's repo.
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatus {
    pub exists: bool,
    pub is_repo: bool,
    pub has_commits: bool,
    pub remote_url: Option<String>,
    /// `owner/repo` (or `group/sub/project`) when the remote is a forge Hangar
    /// knows.
    pub forge_path: Option<String>,
    pub forge: Option<String>,
    pub branch: Option<String>,
    pub has_main: bool,
    pub has_dev: bool,
    /// What a task branch starts from: `dev` when there is one, else `main`.
    pub base_branch: Option<String>,
}

pub fn repo_status(dir: &Path) -> RepoStatus {
    let mut s = RepoStatus { exists: dir.is_dir(), ..Default::default() };
    if !s.exists {
        return s;
    }
    s.is_repo = git(dir, &["rev-parse", "--is-inside-work-tree"], &[]).is_ok_and(|o| o.trim() == "true");
    if !s.is_repo {
        return s;
    }
    s.has_commits = git(dir, &["rev-parse", "--verify", "--quiet", "HEAD"], &[]).is_ok();
    s.branch = git(dir, &["branch", "--show-current"], &[]).ok().map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    let has = |b: &str| git(dir, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")], &[]).is_ok()
        || git(dir, &["rev-parse", "--verify", "--quiet", &format!("refs/remotes/origin/{b}")], &[]).is_ok();
    s.has_main = has("main");
    s.has_dev = has("dev");
    s.base_branch = if s.has_dev { Some("dev".into()) } else if s.has_main { Some("main".into()) } else { s.branch.clone() };
    s.remote_url = git(dir, &["remote", "get-url", "origin"], &[]).ok().map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
    if s.remote_url.is_some() {
        if let Ok(repo) = crate::forges::detect_repo(dir) {
            s.forge_path = Some(repo.path());
            s.forge = Some(format!("{:?}", repo.forge).to_lowercase());
        }
    }
    s
}

/// `git init` on `main`, a first commit of whatever is there (an empty one for
/// an empty folder) and `dev` beside it — the base every task branches from.
pub fn init_repo(dir: &Path, env: &[(String, String)]) -> Result<(), String> {
    // Its own `.git`, not "inside a work tree": a new project under a folder
    // that happens to be a repo still gets a repo of its own.
    if dir.join(".git").exists() {
        return Err("this folder is already a git repo".to_string());
    }
    git(dir, &["init", "-b", "main"], env)?;
    git(dir, &["add", "-A"], env)?;
    git(dir, &["commit", "--allow-empty", "-m", "Initial commit"], env)
        .map_err(|e| format!("git init worked but the first commit didn't ({e}) — set your git name and email (Settings → Git identity)"))?;
    git(dir, &["branch", "dev"], env)?;
    Ok(())
}

/// The folder for a new project: `parent/name`, which must not exist yet (or
/// be empty). Created here; the repo comes after.
pub fn new_folder(parent: &str, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return Err("give the project a folder name without slashes".to_string());
    }
    let parent = expand_home(parent.trim());
    if !parent.is_dir() {
        return Err(format!("{} isn't a folder", parent.display()));
    }
    let dir = parent.join(name);
    if dir.exists() && std::fs::read_dir(&dir).map(|mut d| d.next().is_some()).unwrap_or(true) {
        return Err(format!("{} already exists and isn't empty — choose «Carpeta existente» for it", dir.display()));
    }
    std::fs::create_dir_all(&dir).map_err(|e| format!("couldn't create {}: {e}", dir.display()))?;
    Ok(dir)
}

/// A new project's folder with its repo, or nothing at all: when `git init`
/// fails (no git identity, say) the folder is taken back, so trying again
/// isn't refused as "already exists".
pub fn create_repo_folder(parent: &str, name: &str) -> Result<PathBuf, String> {
    let dir = new_folder(parent, name)?;
    if let Err(e) = init_repo(&dir, &[]) {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(dir)
}

/// `~/Proyects` → the home folder's `Proyects`.
pub fn expand_home(path: &str) -> PathBuf {
    crate::app_data::expand_home(path, crate::app_data::home_dir().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: [(&str, &str); 4] = [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t")];

    fn id() -> Vec<(String, String)> {
        ID.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn temp(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hangar-factory-{label}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_new_folder_becomes_a_repo_with_main_and_dev() {
        let parent = temp("parent");
        let dir = new_folder(&parent.to_string_lossy(), "citas-vet").unwrap();
        init_repo(&dir, &id()).unwrap();
        let s = repo_status(&dir);
        assert!(s.is_repo && s.has_commits && s.has_main && s.has_dev);
        assert_eq!((s.branch.as_deref(), s.base_branch.as_deref(), s.remote_url.as_deref()), (Some("main"), Some("dev"), None));
        assert!(init_repo(&dir, &id()).is_err(), "never twice");
        assert!(new_folder(&parent.to_string_lossy(), "citas-vet").is_err(), "it isn't empty any more");
        let _ = std::fs::remove_dir_all(&parent);
    }

    #[test]
    fn an_existing_folder_keeps_its_files_in_the_first_commit() {
        let dir = temp("existing");
        std::fs::write(dir.join("README.md"), "hola\n").unwrap();
        init_repo(&dir, &id()).unwrap();
        let files = git(&dir, &["ls-files"], &[]).unwrap();
        assert_eq!(files.trim(), "README.md");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_remote_says_which_forge_and_repo() {
        let dir = temp("remote");
        init_repo(&dir, &id()).unwrap();
        git(&dir, &["remote", "add", "origin", "https://github.com/acme/citas-vet.git"], &[]).unwrap();
        let s = repo_status(&dir);
        assert_eq!(s.remote_url.as_deref(), Some("https://github.com/acme/citas-vet.git"));
        assert_eq!((s.forge_path.as_deref(), s.forge.as_deref()), (Some("acme/citas-vet"), Some("github")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_that_are_not_repos_or_do_not_exist_say_so() {
        let dir = temp("plain");
        let s = repo_status(&dir);
        assert!(s.exists && !s.is_repo);
        assert!(!repo_status(&dir.join("missing")).exists);
        for bad in ["", "a/b", ".hidden"] {
            assert!(new_folder(&dir.to_string_lossy(), bad).is_err(), "{bad:?}");
        }
        assert!(new_folder(&dir.join("nope").to_string_lossy(), "x").is_err(), "the parent must exist");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_project_inside_another_repo_gets_its_own() {
        let outer = temp("outer");
        init_repo(&outer, &id()).unwrap();
        let inner = new_folder(&outer.to_string_lossy(), "citas-vet").unwrap();
        assert!(repo_status(&inner).is_repo, "inside the outer work tree");
        init_repo(&inner, &id()).unwrap();
        assert!(inner.join(".git").is_dir());
        let _ = std::fs::remove_dir_all(&outer);
    }
}
