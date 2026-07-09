//! A blocking subprocess runner with a timeout: spawn a command, drain its
//! pipes concurrently, and kill it if it overruns. Used by the tool layer
//! (`run_tests`, git/mission/shell/vision helpers) to shell out safely without
//! a stuck child stalling the REPL.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Raw result from a subprocess invocation.
/// with the addition of `exit_code` for finer diagnostics.
#[derive(Debug, Clone)]
pub struct CommandResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub exit_code: Option<i32>,
}

// ────────────────────────────────────────────────────────────────────────────
// Subprocess execution
// ────────────────────────────────────────────────────────────────────────────

/// Spawn `program` with `args`, capturing stdout/stderr. Polls
/// `child.try_wait()` every 100 ms and kills the process if the timeout
/// is exceeded. Returns a `CommandResult` in all cases — never panics.
///
/// `cwd` overrides the subprocess working directory — callers that must run
/// the command from a specific directory (e.g. a project root so the test
/// framework resolves its config) pass it here.
///
/// **Pipe draining:** stdout and stderr are read concurrently on dedicated
/// reader threads. Reading lazily (only after `try_wait` returns `Some`)
/// deadlocks any child that writes more than the OS pipe buffer (~64 KB)
/// because the child blocks on the write while the parent spins on
/// `try_wait` and the timeout eats the whole 30 s budget.
pub fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    timeout_secs: u64,
    cwd: Option<&Path>,
) -> CommandResult {
    let mut cmd = Command::new(program);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandResult {
                success: false,
                stdout: String::new(),
                stderr: format!("failed to spawn `{program}`: {e}"),
                timed_out: false,
                exit_code: None,
            };
        }
    };

    // Drain pipes on background threads so the child can't block writing.
    let stdout_reader = spawn_pipe_reader(child.stdout.take());
    let stderr_reader = spawn_pipe_reader(child.stderr.take());

    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return CommandResult {
                    success: status.success(),
                    stdout: join_reader(stdout_reader),
                    stderr: join_reader(stderr_reader),
                    timed_out: false,
                    exit_code: status.code(),
                };
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Reader threads exit cleanly once the kill closes the
                    // pipe ends; join so any partial output is still
                    // returned with the timeout message appended.
                    let stdout = join_reader(stdout_reader);
                    let mut stderr = join_reader(stderr_reader);
                    if !stderr.is_empty() && !stderr.ends_with('\n') {
                        stderr.push('\n');
                    }
                    use std::fmt::Write as _;
                    let _ = write!(stderr, "timed out after {timeout_secs}s");
                    return CommandResult {
                        success: false,
                        stdout,
                        stderr,
                        timed_out: true,
                        exit_code: None,
                    };
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => {
                return CommandResult {
                    success: false,
                    stdout: join_reader(stdout_reader),
                    stderr: format!(
                        "{}\ntry_wait error: {e}",
                        join_reader(stderr_reader).trim_end()
                    ),
                    timed_out: false,
                    exit_code: None,
                };
            }
        }
    }
}

/// Spawn a background thread that drains a child pipe into a `String`. The
/// thread exits when the pipe closes (child exit or kill). `None` input
/// yields a thread that returns an empty string immediately so callers
/// never have to special-case "pipe wasn't captured."
fn spawn_pipe_reader<P>(pipe: Option<P>) -> std::thread::JoinHandle<String>
where
    P: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let Some(mut r) = pipe else {
            return String::new();
        };
        let mut buf = String::new();
        let _ = r.read_to_string(&mut buf);
        buf
    })
}

/// Join a pipe-reader thread and return its captured string. Treats a
/// panicked reader as an empty pipe — output capture is best-effort.
fn join_reader(handle: std::thread::JoinHandle<String>) -> String {
    handle.join().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_command_drains_large_output_without_timeout() {
        // Regression: previously the parent only read pipes after the child
        // exited, so any child that wrote more than the OS pipe buffer
        // (~64 KB) would block on write and we would burn the full timeout.
        // With concurrent drainers the child runs to completion.
        //
        // The regression is captured by `!timed_out` + `success` +
        // `stdout.len() == 200_000`: a non-concurrent drain would block the
        // child on its 200 KB write (well over the pipe buffer), the 10 s
        // timeout would fire, and `timed_out`/`success` would flip. We do
        // NOT assert an absolute wall-clock bound — python cold-start plus
        // scheduling jitter on a loaded CI runner makes that flaky without
        // adding signal the timeout guard doesn't already give.
        //
        // Python is the most portable "spew 200 KB" we have on the runners
        // that already cover the rest of this module. If python isn't
        // installed we skip — the assertion is meaningful only when the
        // subprocess actually ran.
        let body = "import sys; sys.stdout.write('x' * 200_000); sys.stdout.flush()";
        let result = run_command_with_timeout("python", &["-c", body], 10, None);
        if !result.success
            && result.exit_code.is_none()
            && result.stderr.starts_with("failed to spawn")
        {
            eprintln!("skipping: python not on PATH");
            return;
        }
        assert!(!result.timed_out, "should not time out: {result:?}");
        assert!(result.success, "child should exit 0: {result:?}");
        assert_eq!(result.stdout.len(), 200_000);
    }
}
