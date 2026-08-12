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

/// [`run_with_timeout`] for a command that has to be *fed* a secret.
///
/// A sibling rather than a parameter on the original, which pins
/// `.stdin(Stdio::null())` and is called from a few dozen places that would all
/// have to grow an argument they never use.
///
/// **Why stdin at all.** A token passed as an argument is readable by every
/// other process on the machine for as long as the command runs (`ps` shows the
/// full command line), and lands in any shell history or crash log that captures
/// it. `gh auth login --with-token` and `glab auth login --stdin` both exist for
/// this reason; this is the plumbing that lets Hangar use them.
///
/// The secret is written and the pipe closed before waiting, because these CLIs
/// read to EOF — leaving the handle open deadlocks the very timeout meant to
/// catch a hang.
pub fn run_with_stdin(mut cmd: Command, secret: &str, timeout: Duration) -> Result<Output, String> {
    use std::io::Write;

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).stdin(Stdio::piped());
    let mut child: Child = cmd.spawn().map_err(|e| format!("failed to start process: {e}"))?;
    let pid = child.id();

    // Taken, not borrowed: dropping the handle is what closes the pipe, and the
    // child waits for EOF before it does anything.
    match child.stdin.take() {
        Some(mut pipe) => {
            // The write can fail if the child died first (a missing subcommand,
            // say). That is not the interesting error — the child's own stderr
            // is — so it is not propagated here.
            let _ = pipe.write_all(secret.as_bytes());
            let _ = pipe.write_all(b"\n");
        }
        None => {
            kill_by_pid(pid);
            return Err("could not open a pipe to the process".to_string());
        }
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    match rx.recv_timeout(timeout) {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(e)) => Err(format!("process failed: {e}")),
        Err(_) => {
            kill_by_pid(pid);
            Err(format!("timed out after {}s — check your network connection", timeout.as_secs()))
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

    /// The secret reaches the child, and reaches it *whole* — `cat` only ever
    /// returns once the pipe is closed, so this passing at all is the proof
    /// that the handle is dropped rather than held.
    #[test]
    fn a_secret_reaches_the_child_over_stdin() {
        let output = run_with_stdin(Command::new("cat"), "glpat-not-a-real-token", Duration::from_secs(5)).unwrap();

        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "glpat-not-a-real-token");
    }

    /// The whole reason this exists: the token must not be visible in the
    /// process's own command line, which is world-readable through `ps` for as
    /// long as it runs.
    #[test]
    fn the_secret_never_appears_in_the_arguments() {
        let mut cmd = Command::new("ps");
        cmd.arg("-o").arg("args=").arg("-p").arg(std::process::id().to_string());

        let output = run_with_stdin(cmd, "glpat-not-a-real-token", Duration::from_secs(5)).unwrap();

        assert!(!String::from_utf8_lossy(&output.stdout).contains("glpat-"), "a token in argv is readable by every process on the machine");
    }

    /// The timeout discipline has to survive the added plumbing — a child that
    /// never reads its stdin must still be killed rather than hang the caller.
    #[test]
    fn a_child_that_ignores_its_stdin_is_still_killed_on_time() {
        let mut cmd = Command::new("sleep");
        cmd.arg("5");
        let start = std::time::Instant::now();

        let result = run_with_stdin(cmd, "secret", Duration::from_millis(300));

        assert!(result.is_err_and(|e| e.contains("timed out")));
        assert!(start.elapsed() < Duration::from_secs(2), "the stdin variant must bound the wait exactly as the plain one does");
    }
}
