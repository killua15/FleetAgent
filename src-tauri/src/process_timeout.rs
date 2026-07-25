//! A hard ceiling on subprocess calls that talk to the outside world (`git`
//! fetch/push, `gh` API calls, `claude` auth checks) — none of these have a
//! built-in timeout, so a dead network, a hung SSH passphrase prompt, or a
//! CLI needing interactive re-auth blocks the calling `run_blocking` thread
//! forever: the button just spins with no way out. `run_with_timeout` bounds
//! that wait and kills the process by pid if it's exceeded — the exact same
//! kill-by-pid technique `turn_runner::kill_turn` already uses to stop a
//! hung agent turn, just applied here to short-lived one-shot commands.
//!
//! Deliberately NOT used for: agent turn processes in `turn_runner.rs` (they
//! run for minutes by design and already have their own kill path), or the
//! user's arbitrary `setup_command` in `sessions.rs` (a real `npm install`
//! can legitimately take a while — it needs a much longer bound, a separate
//! decision from this one).

use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::Duration;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(25);

fn kill_by_pid(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/F"]).output();
    }
}

pub fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::null());
    let child: Child = cmd.spawn().map_err(|e| format!("failed to start process: {e}"))?;
    let pid = child.id();

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("process failed: {e}")),
        Err(_) => {
            kill_by_pid(pid);
            Err(format!(
                "timed out after {}s — check your network connection, or whether git/gh needs you to re-authenticate",
                timeout.as_secs()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_the_real_output_of_a_fast_command() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let output = run_with_timeout(cmd, Duration::from_secs(5)).unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
    }

    #[test]
    fn kills_and_errors_on_a_command_that_outlives_the_timeout() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let start = std::time::Instant::now();
        let result = run_with_timeout(cmd, Duration::from_millis(300));
        assert!(result.is_err(), "a command that outlives the timeout must error, not hang the caller");
        assert!(result.unwrap_err().contains("timed out"));
        assert!(start.elapsed() < Duration::from_secs(2), "must return promptly after the timeout, not wait for the full sleep");
    }
}
