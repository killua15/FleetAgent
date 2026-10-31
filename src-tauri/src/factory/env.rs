//! The environment the line's commands run in. An app opened from the Finder
//! inherits a bare PATH (`/usr/bin:/bin:…`), so `pnpm`, `node` or `pytest`
//! installed through Homebrew, nvm or a version manager aren't found by the
//! gate, by the app the UI reviewers open, or by the agents' own Bash — even
//! though a terminal finds them. The user's login shell knows where they are.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::OnceLock;
use std::time::Duration;

static USER_PATH: OnceLock<String> = OnceLock::new();

/// The login shell's PATH, then the process's own, without repeats. Asked once.
pub fn user_path() -> String {
    USER_PATH
        .get_or_init(|| {
            let own = std::env::var("PATH").unwrap_or_default();
            let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
            let mut cmd = Command::new(shell);
            // `-i` too: nvm and friends are set up in .zshrc, which only an
            // interactive shell reads.
            cmd.args(["-ilc", "printf '%s' \"$PATH\""]);
            let login = crate::process_timeout::run_with_timeout(cmd, Duration::from_secs(10))
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or("").trim().to_string())
                .unwrap_or_default();
            merge(&login, &own)
        })
        .clone()
}

fn merge(first: &str, second: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    first.split(':').chain(second.split(':')).filter(|d| !d.is_empty() && seen.insert(d.to_string())).collect::<Vec<_>>().join(":")
}

/// A shell command in `dir`, with the user's PATH — tracked under `project`,
/// so ⏹ kills it and whatever it started.
pub fn shell(project: &str, key: &str, command: &str, dir: &Path, timeout: Duration) -> Result<Output, String> {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", command]).current_dir(dir).env("PATH", user_path());
    super::procs::run(project, key, cmd, timeout)
}

/// Works out, once and off any critical path, what the Factory would
/// otherwise work out the first time it needs it: the login shell's PATH and
/// where `claude` and `codex` are. Called at startup on a thread of its own.
pub fn warm_up() {
    std::thread::spawn(|| {
        let _ = user_path();
        let _ = crate::providers::claude::binary();
        let _ = crate::providers::resolve_binary("codex");
    });
}

/// Whether an executable is on the user's PATH.
pub fn has_executable(name: &str) -> bool {
    std::env::split_paths(&user_path()).any(|d| {
        let p = d.join(name);
        p.is_file()
    })
}

/// The program a gate command runs first: `pnpm` out of `pnpm test`, `grep`
/// out of `! grep -rn …`, nothing out of `test …` (a shell builtin).
pub fn first_program(command: &str) -> Option<String> {
    const BUILTINS: [&str; 12] = ["test", "[", "!", "true", "false", "cd", "echo", "printf", "if", "for", "export", "set"];
    let mut words = command.split_whitespace().map(|w| w.trim_start_matches(['(', '{']));
    loop {
        let w = words.next()?;
        if w.is_empty() || w == "!" || w.contains('=') && !w.starts_with('=') && !w.contains('/') {
            continue;
        }
        return (!BUILTINS.contains(&w)).then(|| w.to_string());
    }
}

/// Installs a worktree's dependencies with whatever its lockfile says, so the
/// tests and the checks can run. `None` when there's nothing to install.
pub fn install_deps(project: &str, dir: &Path) -> Option<Result<String, String>> {
    let (cmd, label) = if dir.join("pnpm-lock.yaml").exists() {
        ("pnpm install --frozen-lockfile", "pnpm install")
    } else if dir.join("package-lock.json").exists() {
        ("npm ci", "npm ci")
    } else if dir.join("yarn.lock").exists() {
        ("yarn install --frozen-lockfile", "yarn install")
    } else if dir.join("bun.lockb").exists() || dir.join("bun.lock").exists() {
        ("bun install --frozen-lockfile", "bun install")
    } else if dir.join("uv.lock").exists() {
        ("uv sync", "uv sync")
    } else if dir.join("poetry.lock").exists() {
        ("poetry install", "poetry install")
    } else {
        return None;
    };
    Some(match shell(project, "install", cmd, dir, Duration::from_secs(15 * 60)) {
        Ok(o) if o.status.success() => Ok(label.to_string()),
        Ok(o) => Err(format!("{label} falló: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""))),
        Err(e) => Err(format!("{label}: {e}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_program_of_a_gate_command() {
        assert_eq!(first_program("pnpm test").as_deref(), Some("pnpm"));
        assert_eq!(first_program("! grep -rn \"session.query\" app").as_deref(), Some("grep"));
        assert_eq!(first_program("CI=1 pnpm e2e").as_deref(), Some("pnpm"));
        assert_eq!(first_program("test \"$(cat .nvmrc)\" = \"24\" && node -v"), None);
    }

    #[test]
    fn the_login_path_comes_first_and_nothing_repeats() {
        assert_eq!(merge("/opt/homebrew/bin:/usr/bin", "/usr/bin:/bin"), "/opt/homebrew/bin:/usr/bin:/bin");
        let path = user_path();
        assert!(path.split(':').any(|d| d == "/usr/bin"));
    }
}
