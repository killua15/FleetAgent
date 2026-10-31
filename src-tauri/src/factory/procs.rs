//! Every process the Factory starts — the agents, the gate's commands,
//! dependency installs, the app the UI reviewers open, the full verification —
//! runs in a process group of its own and is registered under its project, so
//! ⏹ stops all of it at once: `pnpm`, `vite`, Playwright and its browsers die
//! with the command that started them, instead of living on and eating CPU.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::process::{Child, Command, Output, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Project → (process group id → what it is, e.g. `run-123` or `gate`).
static GROUPS: LazyLock<Mutex<HashMap<String, HashMap<u32, String>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// What ⏹ was pressed for: a project id, or `project/key` for one job of it
/// (the TRD analysis). Looked at, not consumed: everything of the project
/// stops, and ▶ clears it.
static STOPPED: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(|| Mutex::new(HashSet::new()));

const GRACE: Duration = Duration::from_secs(2);

/// A running process, unregistered when dropped.
pub struct Tracked {
    pub child: Child,
    project: String,
    pid: u32,
}

impl Tracked {
    pub fn id(&self) -> u32 {
        self.pid
    }
}

impl Drop for Tracked {
    fn drop(&mut self) {
        unregister(&self.project, self.pid);
    }
}

fn unregister(project: &str, pid: u32) {
    if let Ok(mut g) = GROUPS.lock() {
        if let Some(p) = g.get_mut(project) {
            p.remove(&pid);
            if p.is_empty() {
                g.remove(project);
            }
        }
    }
}

/// Starts `cmd` as the leader of a new process group, registered under
/// `project` as `key`. Refused when ⏹ was already pressed for it.
pub fn spawn(project: &str, key: &str, cmd: &mut Command) -> Result<Tracked, String> {
    if stopped(project, key) {
        return Err("parado".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let child = cmd.spawn().map_err(|e| format!("no se pudo arrancar el proceso: {e}"))?;
    let pid = child.id();
    if let Ok(mut g) = GROUPS.lock() {
        g.entry(project.to_string()).or_default().insert(pid, key.to_string());
    }
    Ok(Tracked { child, project: project.to_string(), pid })
}

/// Runs `cmd` to the end like `Command::output`, tracked, killed on timeout
/// or on ⏹. Errors say which.
pub fn run(project: &str, key: &str, mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut t = spawn(project, key, &mut cmd)?;
    // Read both pipes on their own threads, or a chatty command fills one and
    // blocks forever.
    let mut out = t.child.stdout.take();
    let mut err = t.child.stderr.take();
    let o = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = out.as_mut() {
            let _ = s.read_to_end(&mut b);
        }
        b
    });
    let e = std::thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(s) = err.as_mut() {
            let _ = s.read_to_end(&mut b);
        }
        b
    });
    let started = Instant::now();
    let status = loop {
        match t.child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(e) => break Err(e.to_string()),
        }
        if stopped(project, key) {
            kill_group(t.pid);
            let _ = t.child.wait();
            break Err("parado".into());
        }
        if started.elapsed() > timeout {
            kill_group(t.pid);
            let _ = t.child.wait();
            break Err(format!("sin terminar tras {} min", timeout.as_secs() / 60));
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    // The group can outlive its leader (a server it started); take it down.
    kill_group_now(t.pid);
    let stdout = o.join().unwrap_or_default();
    let stderr = e.join().unwrap_or_default();
    // ⏹ kills from another thread: the process may have exited (killed)
    // before this loop looked at the flag. Killed by ⏹ is «parado» all the same.
    if stopped(project, key) {
        return Err("parado".into());
    }
    status.map(|status| Output { status, stdout, stderr })
}

/// TERM to the whole group, KILL to what's left after a grace period.
pub fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        let pgid = -(pid as i32);
        // SAFETY: plain syscall; a group that's already gone just returns ESRCH.
        unsafe { libc::kill(pgid, libc::SIGTERM) };
        let until = Instant::now() + GRACE;
        while Instant::now() < until {
            if unsafe { libc::kill(pgid, 0) } != 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        unsafe { libc::kill(pgid, libc::SIGKILL) };
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
    }
}

/// KILL to whatever of the group is still alive, without waiting.
fn kill_group_now(pid: u32) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(pid as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    let _ = pid;
}

fn stopped(project: &str, key: &str) -> bool {
    STOPPED.lock().map(|s| s.contains(project) || s.contains(&format!("{project}/{key}"))).unwrap_or(false)
}

/// Whether ⏹ was pressed for the project (and not cleared by ▶ since).
pub fn stop_requested(project: &str) -> bool {
    STOPPED.lock().map(|s| s.contains(project)).unwrap_or(false)
}

/// Whether one job of the project was cancelled, or the whole project stopped.
pub fn job_stopped(project: &str, key: &str) -> bool {
    stopped(project, key)
}

/// ⏹: marks the project stopped and kills every process group it has, in the
/// background — the caller returns at once. Returns how many there were.
pub fn stop_project(project: &str) -> usize {
    if let Ok(mut s) = STOPPED.lock() {
        s.insert(project.to_string());
    }
    kill_where(|p, _| p == project)
}

/// Cancels one job (`plan`) of a project, leaving the rest running.
pub fn stop_job(project: &str, key: &str) -> usize {
    if let Ok(mut s) = STOPPED.lock() {
        s.insert(format!("{project}/{key}"));
    }
    kill_where(|p, k| p == project && k == key)
}

fn kill_where(pick: impl Fn(&str, &str) -> bool) -> usize {
    let pids: Vec<u32> = GROUPS
        .lock()
        .map(|g| g.iter().flat_map(|(p, groups)| groups.iter().filter(|(_, k)| pick(p, k)).map(|(pid, _)| *pid).collect::<Vec<_>>()).collect())
        .unwrap_or_default();
    let n = pids.len();
    if n > 0 {
        std::thread::spawn(move || {
            for pid in pids {
                kill_group(pid);
            }
        });
    }
    n
}

/// ▶ (or a new analysis) forgets an earlier ⏹.
pub fn clear_stop(project: &str) {
    if let Ok(mut s) = STOPPED.lock() {
        s.retain(|k| k != project && !k.starts_with(&format!("{project}/")));
    }
}

pub fn clear_job(project: &str, key: &str) {
    if let Ok(mut s) = STOPPED.lock() {
        s.remove(&format!("{project}/{key}"));
    }
}

/// Projects with a process alive right now.
pub fn busy_projects() -> Vec<String> {
    GROUPS.lock().map(|g| g.keys().cloned().collect()).unwrap_or_default()
}

/// On quit: nothing of the Factory outlives Hangar.
pub fn kill_everything() {
    let pids: Vec<u32> = GROUPS.lock().map(|g| g.values().flat_map(|p| p.keys().copied()).collect()).unwrap_or_default();
    for pid in pids {
        kill_group_now(pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alive_in_group(pgid: u32) -> bool {
        #[cfg(unix)]
        {
            unsafe { libc::kill(-(pgid as i32), 0) == 0 }
        }
        #[cfg(not(unix))]
        {
            let _ = pgid;
            false
        }
    }

    #[cfg(unix)]
    #[test]
    fn stop_takes_down_the_command_and_what_it_started() {
        let project = format!("p-{}", uuid::Uuid::new_v4().simple());
        let mut cmd = Command::new("sh");
        // A background child, like a dev server, besides the command itself.
        cmd.args(["-c", "sleep 60 & sleep 60"]);
        let t = spawn(&project, "gate", &mut cmd).unwrap();
        let pgid = t.id();
        std::thread::sleep(Duration::from_millis(200));
        assert!(alive_in_group(pgid));
        assert_eq!(stop_project(&project), 1);
        let until = Instant::now() + Duration::from_secs(5);
        while alive_in_group(pgid) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(!alive_in_group(pgid), "the background sleep died too");
        assert!(stop_requested(&project));
        assert!(spawn(&project, "gate", &mut Command::new("true")).is_err(), "nothing new starts while stopped");
        clear_stop(&project);
        assert!(!stop_requested(&project));
        drop(t);
    }

    #[cfg(unix)]
    #[test]
    fn run_is_killed_by_stop_and_by_its_timeout() {
        let project = format!("p-{}", uuid::Uuid::new_v4().simple());
        let p2 = project.clone();
        let stopper = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            stop_project(&p2);
        });
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let started = Instant::now();
        assert_eq!(run(&project, "install", cmd, Duration::from_secs(60)).unwrap_err(), "parado");
        assert!(started.elapsed() < Duration::from_secs(5));
        stopper.join().unwrap();
        clear_stop(&project);

        let mut slow = Command::new("sh");
        slow.args(["-c", "sleep 30"]);
        assert!(run(&project, "gate", slow, Duration::from_millis(300)).unwrap_err().contains("sin terminar"));
        let mut ok = Command::new("sh");
        ok.args(["-c", "echo hola; echo mal >&2; exit 3"]);
        let out = run(&project, "gate", ok, Duration::from_secs(10)).unwrap();
        assert_eq!((String::from_utf8_lossy(&out.stdout).trim(), out.status.code()), ("hola", Some(3)));
        assert!(busy_projects().iter().all(|p| p != &project), "finished processes leave the registry");
    }

    #[cfg(unix)]
    #[test]
    fn cancelling_one_job_leaves_the_rest() {
        let project = format!("p-{}", uuid::Uuid::new_v4().simple());
        let plan = spawn(&project, "plan", Command::new("sleep").arg("30")).unwrap();
        let other = spawn(&project, "run-1", Command::new("sleep").arg("30")).unwrap();
        stop_job(&project, "plan");
        std::thread::sleep(Duration::from_millis(300));
        assert!(!alive_in_group(plan.id()));
        assert!(alive_in_group(other.id()));
        assert!(job_stopped(&project, "plan") && !stop_requested(&project));
        stop_project(&project);
        clear_stop(&project);
    }
}
