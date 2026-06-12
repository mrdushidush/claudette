//! Shell + edit group — 5 tools: bash (sync, 30 s cap), edit_file
//! (legacy single-replace), and the v0.6.0 background-job family
//! (bash_background, bash_status, bash_tail).
//!
//! These are the DangerFullAccess tools: bash can run arbitrary shell
//! commands; edit_file can modify files under the user's $HOME (broader
//! than write_file's ~/.claudette/files/ sandbox). The background-job
//! family inherits the same gate — `bash_background` accepts the same
//! confirmation up-front; subsequent `bash_status` / `bash_tail` reads
//! are auto-allowed.
//!
//! Background-job storage layout (`~/.claudette/jobs/`):
//!   <id>.meta — JSON {job_id, pid, cmd, cwd, started_at}
//!   <id>.out  — captured stdout
//!   <id>.err  — captured stderr
//!   <id>.done — written by the reaper thread on child exit:
//!               first line = exit code, second line = ended_at RFC3339.
//!   No index.json — listings derive from <id>.meta globs at query time,
//!   which avoids the multi-job write race the brief flagged.
//!
//! Self-contained: `BASH_OUTPUT_MAX_CHARS` is private. Handlers reuse
//! the parent-module `validate_edit_path` (pub(super)) for edit_file's
//! path gate, and `run_command_with_timeout` from crate::test_runner
//! directly for bash's subprocess.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{claudette_home, ensure_dir, parse_json_input, validate_edit_path};
use crate::test_runner::run_command_with_timeout;

const BASH_OUTPUT_MAX_CHARS: usize = 8192;

fn jobs_dir() -> PathBuf {
    claudette_home().join("jobs")
}

/// File quartet for a background job. Computed from the job id; we never
/// fan out — every file uses the same prefix.
fn job_paths(job_id: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let d = jobs_dir();
    (
        d.join(format!("{job_id}.meta")),
        d.join(format!("{job_id}.out")),
        d.join(format!("{job_id}.err")),
        d.join(format!("{job_id}.done")),
    )
}

/// Job ids look like `bg_<unix-millis>` — sortable, human-readable, and
/// unique enough at one-shell-per-millisecond granularity.
fn new_job_id() -> String {
    format!("bg_{}", chrono::Local::now().timestamp_millis())
}

#[derive(Serialize, Deserialize, Clone)]
struct JobMeta {
    job_id: String,
    pid: u32,
    cmd: String,
    cwd: String,
    started_at: String,
}

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Run a shell command (asks for confirmation). PowerShell on Windows (use ; not &&, $env:VAR, backslash paths); sh elsewhere. Prefer `git -C <dir>` / `cargo --manifest-path` over cd-chaining. For anything beyond one command, write_file a script and run that — inline -c/-Command quoting gets mangled.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command" }
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Legacy single-string text replace (one occurrence only). Prefer apply_patch for multi-line / multi-file edits — v0.6.0 marks this for removal in a future release. For new files use write_file or generate_code.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":     { "type": "string", "description": "File path (absolute or ~/)" },
                        "old_text": { "type": "string", "description": "Exact text to find and replace" },
                        "new_text": { "type": "string", "description": "Replacement text" }
                    },
                    "required": ["path", "old_text", "new_text"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "bash_background",
                "description": "Spawn a long-running shell command. Returns {job_id, pid} immediately. Use bash_status + bash_tail to track progress. Output is captured to ~/.claudette/jobs/<id>.{out,err}.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command (same syntax as `bash`)." },
                        "cwd":     { "type": "string", "description": "Optional working directory (defaults to the active mission cwd)." }
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "bash_status",
                "description": "Check a bash_background job's state: 'running' or 'exited' (with exit_code + runtime_ms).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "Job id returned by bash_background." }
                    },
                    "required": ["job_id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "bash_tail",
                "description": "Tail recent output from a bash_background job. Returns the last `lines` (default 100) from the requested stream.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "job_id": { "type": "string", "description": "Job id from bash_background." },
                        "lines":  { "type": "number", "description": "Number of lines per stream (default 100, max 1000)." },
                        "stream": { "type": "string", "description": "'stdout', 'stderr', or 'both' (default)." }
                    },
                    "required": ["job_id"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "bash" => run_bash(input),
        "edit_file" => run_edit_file(input),
        "bash_background" => run_bash_background(input),
        "bash_status" => run_bash_status(input),
        "bash_tail" => run_bash_tail(input),
        _ => return None,
    };
    Some(result)
}

fn run_bash(input: &str) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(input).map_err(|e| format!("bash: invalid JSON ({e}): {input}"))?;
    let command = v
        .get("command")
        .and_then(Value::as_str)
        .ok_or("bash: missing 'command'")?;

    if command.trim().is_empty() {
        return Err("bash: command is empty".to_string());
    }

    // Execute via the platform shell so pipes, redirects, and builtins work.
    // Windows: PowerShell 5.1+ (powershell.exe) — ships with every supported
    // Windows release. cmd.exe is avoided because small-model brains tend to
    // emit Unix-style pipelines that cmd can't parse, and findstr/Select-Object
    // get mixed in the same line. PowerShell is closer to that pre-trained
    // distribution. Flags: -NoProfile (skip $PROFILE), -NonInteractive (fail
    // fast on Read-Host instead of hanging), -Command (single-string).
    #[cfg(target_os = "windows")]
    let (program, args) = (
        "powershell",
        vec!["-NoProfile", "-NonInteractive", "-Command", command],
    );
    #[cfg(not(target_os = "windows"))]
    let (program, args) = ("sh", vec!["-c", command]);

    // bash inherits the active-mission cwd (T2): when the brain is
    // working a brownfield mission, `bash` runs inside that tree so
    // shell-driven workflows (build, test, scripted edits) stay
    // self-consistent with git_*. Falls back to process cwd otherwise.
    let cwd = crate::missions::active_cwd();
    let result = run_command_with_timeout(program, &args, 30, Some(&cwd));

    let stdout: String = result.stdout.chars().take(BASH_OUTPUT_MAX_CHARS).collect();
    let stderr: String = result.stderr.chars().take(BASH_OUTPUT_MAX_CHARS).collect();
    let truncated =
        result.stdout.len() > BASH_OUTPUT_MAX_CHARS || result.stderr.len() > BASH_OUTPUT_MAX_CHARS;

    Ok(json!({
        "exit_code": result.exit_code,
        "stdout": stdout,
        "stderr": stderr,
        "timed_out": result.timed_out,
        "truncated": truncated,
    })
    .to_string())
}

fn run_edit_file(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("edit_file: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'path'")?;
    let old_text = v
        .get("old_text")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'old_text'")?;
    let new_text = v
        .get("new_text")
        .and_then(Value::as_str)
        .ok_or("edit_file: missing 'new_text'")?;

    // Boundary follows the active context (roast RC-B): in the interactive
    // secretary (no mission) this is $HOME-gated, the user having confirmed
    // via the permission prompt; under a forge/brownfield mission it is
    // confined to the mission tree so the autonomous Coder can't edit files
    // outside it (e.g. ~/.ssh/config). See `validate_edit_path`.
    let path = validate_edit_path(path_str)?;

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("edit_file: read {} failed: {e}", path.display()))?;

    // Count occurrences: 0 → clear error, 1 → replace, >1 → refuse instead
    // of silently taking the first match. An ambiguous edit against a
    // large file is the easy way to corrupt it quietly.
    let match_count = content.matches(old_text).count();
    match match_count {
        0 => {
            // Near-miss diagnostics (dogfood T2): point at over-escaped
            // backslashes or the closest drifted window instead of leaving
            // the model to theorize about CRLF/whitespace.
            let hint = super::near_miss::near_miss_hint(&content, old_text)
                .unwrap_or_else(|| "The text to replace must match exactly.".to_string());
            return Err(format!(
                "edit_file: old_text not found in {}. {hint}",
                path.display()
            ));
        }
        1 => {}
        n => {
            return Err(format!(
                "edit_file: old_text appears {n} times in {}. Supply a longer, unique old_text (include surrounding context) so the target is unambiguous.",
                path.display()
            ));
        }
    }

    let new_content = content.replacen(old_text, new_text, 1);

    // Atomic write: serialise to a sibling tmp file, preserve the original
    // file's permissions, then rename. A mid-write crash leaves either the
    // original file intact or the tmp behind for manual recovery — never a
    // truncated target.
    let tmp = path.with_extension("claudette-edit.tmp");
    fs::write(&tmp, &new_content)
        .map_err(|e| format!("edit_file: write {} failed: {e}", tmp.display()))?;
    let perms = fs::metadata(&path).map(|m| m.permissions()).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("edit_file: stat {} failed: {e}", path.display())
    })?;
    fs::set_permissions(&tmp, perms).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("edit_file: chmod {} failed: {e}", tmp.display())
    })?;
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!(
            "edit_file: rename {} -> {} failed: {e}",
            tmp.display(),
            path.display()
        )
    })?;

    let mut result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": new_content.len(),
    });

    // Codet post-edit hook for code files (same as write_file).
    if let Some(validation) = crate::codet::validate_code_file(&path, &[]) {
        result["validation"] = validation.to_json();
        if let crate::codet::CodetStatus::CouldNotFix { ref last_error } = validation.status {
            let short_err: String = last_error.lines().take(3).collect::<Vec<_>>().join(" | ");
            eprintln!(
                "{} {}",
                crate::theme::warn(crate::theme::WARN_GLYPH),
                crate::theme::warn(&format!(
                    "codet: {} failed validation after {} attempt(s), {} landed — {}",
                    path.display(),
                    validation.attempts_made,
                    validation.fixes_applied,
                    short_err,
                ))
            );
        }
    }

    Ok(result.to_string())
}

// ────── background-job family ────────────────────────────────────────────

fn run_bash_background(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "bash_background")?;
    let command = v
        .get("command")
        .and_then(Value::as_str)
        .ok_or("bash_background: missing 'command'")?;
    if command.trim().is_empty() {
        return Err("bash_background: command is empty".to_string());
    }
    let cwd = v
        .get("cwd")
        .and_then(Value::as_str)
        .map_or_else(crate::missions::active_cwd, PathBuf::from);

    ensure_dir(&jobs_dir())?;
    let job_id = new_job_id();
    let (meta_path, out_path, err_path, done_path) = job_paths(&job_id);

    let out_file = fs::File::create(&out_path)
        .map_err(|e| format!("bash_background: open {} failed: {e}", out_path.display()))?;
    let err_file = fs::File::create(&err_path)
        .map_err(|e| format!("bash_background: open {} failed: {e}", err_path.display()))?;

    // Same platform-specific shell selection as the sync `bash` tool —
    // brain-emitted commands should behave identically across sync and
    // background invocations.
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("powershell");
    #[cfg(target_os = "windows")]
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);

    #[cfg(not(target_os = "windows"))]
    let mut cmd = std::process::Command::new("sh");
    #[cfg(not(target_os = "windows"))]
    cmd.args(["-c", command]);

    cmd.current_dir(&cwd)
        .stdout(out_file)
        .stderr(err_file)
        .stdin(std::process::Stdio::null());

    let child = cmd
        .spawn()
        .map_err(|e| format!("bash_background: spawn failed: {e}"))?;
    let pid = child.id();

    let meta = JobMeta {
        job_id: job_id.clone(),
        pid,
        cmd: command.to_string(),
        cwd: cwd.display().to_string(),
        started_at: chrono::Local::now().to_rfc3339(),
    };
    fs::write(
        &meta_path,
        serde_json::to_string_pretty(&meta).unwrap_or_default(),
    )
    .map_err(|e| format!("bash_background: write meta failed: {e}"))?;

    // Reaper thread: wait on the child and stamp the .done file when it
    // exits. We deliberately move `child` into the thread so the parent
    // can return immediately without dropping the handle (which on some
    // platforms would orphan the process).
    let done_path_thread = done_path;
    std::thread::spawn(move || {
        let exit_code = child_wait_code(child);
        let ended = chrono::Local::now().to_rfc3339();
        let _ = fs::write(&done_path_thread, format!("{exit_code}\n{ended}\n"));
    });

    Ok(json!({
        "job_id": job_id,
        "pid": pid,
    })
    .to_string())
}

fn child_wait_code(mut child: std::process::Child) -> i32 {
    match child.wait() {
        Ok(status) => status.code().unwrap_or(-1),
        Err(_) => -1,
    }
}

fn run_bash_status(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "bash_status")?;
    let job_id = v
        .get("job_id")
        .and_then(Value::as_str)
        .ok_or("bash_status: missing 'job_id'")?;

    let (meta_path, _, _, done_path) = job_paths(job_id);
    let meta: JobMeta = serde_json::from_str(
        &fs::read_to_string(&meta_path)
            .map_err(|_| format!("bash_status: no job with id '{job_id}'"))?,
    )
    .map_err(|e| format!("bash_status: meta parse failed: {e}"))?;

    let now = chrono::Local::now();
    let started = chrono::DateTime::parse_from_rfc3339(&meta.started_at)
        .map_or(now, |d| d.with_timezone(&chrono::Local));

    let (state, exit_code, ended_at) = if done_path.exists() {
        let body = fs::read_to_string(&done_path).unwrap_or_default();
        let mut iter = body.lines();
        let code = iter.next().and_then(|l| l.trim().parse::<i32>().ok());
        let ended = iter.next().map(str::to_string);
        ("exited", code, ended)
    } else {
        ("running", None, None)
    };

    let end_time = ended_at
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map_or(now, |d| d.with_timezone(&chrono::Local));
    let runtime_ms = (end_time - started).num_milliseconds().max(0);

    Ok(json!({
        "job_id": job_id,
        "state": state,
        "exit_code": exit_code,
        "runtime_ms": runtime_ms,
        "pid": meta.pid,
        "cmd": meta.cmd,
        "cwd": meta.cwd,
        "started_at": meta.started_at,
        "ended_at": ended_at,
    })
    .to_string())
}

fn run_bash_tail(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "bash_tail")?;
    let job_id = v
        .get("job_id")
        .and_then(Value::as_str)
        .ok_or("bash_tail: missing 'job_id'")?;
    let limit = v
        .get("lines")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .min(1000) as usize;
    let stream = v.get("stream").and_then(Value::as_str).unwrap_or("both");

    let (meta_path, out_path, err_path, _) = job_paths(job_id);
    if !meta_path.exists() {
        return Err(format!("bash_tail: no job with id '{job_id}'"));
    }

    let want_out = stream == "stdout" || stream == "both";
    let want_err = stream == "stderr" || stream == "both";
    if !want_out && !want_err && !stream.is_empty() {
        return Err(format!(
            "bash_tail: unknown stream '{stream}' — use 'stdout', 'stderr', or 'both'"
        ));
    }
    let stdout = if want_out {
        tail_file(&out_path, limit)
    } else {
        Vec::new()
    };
    let stderr = if want_err {
        tail_file(&err_path, limit)
    } else {
        Vec::new()
    };

    Ok(json!({
        "job_id": job_id,
        "stream": stream,
        "stdout": stdout,
        "stderr": stderr,
    })
    .to_string())
}

fn tail_file(path: &Path, n: usize) -> Vec<String> {
    let s = fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = s.lines().collect();
    let start = all.len().saturating_sub(n);
    all[start..].iter().map(|s| (*s).to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_rejects_missing_command() {
        let err = run_bash("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
        assert!(err.contains("command"), "got: {err}");
    }

    #[test]
    fn bash_rejects_empty_command() {
        let err = run_bash(r#"{"command":""}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn bash_rejects_whitespace_only_command() {
        let err = run_bash(r#"{"command":"   "}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn edit_file_rejects_missing_path() {
        let err = run_edit_file(r#"{"old_text":"a","new_text":"b"}"#).unwrap_err();
        assert!(err.contains("missing 'path'"), "got: {err}");
    }

    #[test]
    fn edit_file_rejects_missing_old_text() {
        let err = run_edit_file(r#"{"path":"~/x.txt","new_text":"b"}"#).unwrap_err();
        assert!(err.contains("missing 'old_text'"), "got: {err}");
    }

    #[test]
    fn edit_file_rejects_missing_new_text() {
        let err = run_edit_file(r#"{"path":"~/x.txt","old_text":"a"}"#).unwrap_err();
        assert!(err.contains("missing 'new_text'"), "got: {err}");
    }

    fn home_join(label: &str) -> String {
        // $HOME-rooted so validate_read_path accepts it; unique suffix avoids
        // races between parallel tests.
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".into());
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        format!("{home}/claudette-edit-{label}-{nanos}.txt")
    }

    #[test]
    fn edit_file_errors_on_ambiguous_match() {
        let path = home_join("ambig");
        let original = "alpha\nalpha\nbeta\n";
        fs::write(&path, original).unwrap();

        let input = json!({"path": &path, "old_text": "alpha", "new_text": "X"}).to_string();
        let result = run_edit_file(&input);
        let after = fs::read_to_string(&path).ok();
        let _ = fs::remove_file(&path);

        let err = result.expect_err("expected ambiguity error");
        assert!(
            err.contains("appears") && err.contains("times"),
            "expected ambiguity error, got: {err}"
        );
        assert_eq!(
            after.as_deref(),
            Some(original),
            "file must not change on ambiguous match"
        );
    }

    #[test]
    fn edit_file_replaces_unique_match() {
        let path = home_join("unique");
        fs::write(&path, "one\ntwo\nthree\n").unwrap();

        let input = json!({"path": &path, "old_text": "two", "new_text": "TWO"}).to_string();
        let result = run_edit_file(&input);
        let after = fs::read_to_string(&path).ok();
        let _ = fs::remove_file(&path);

        assert!(result.is_ok(), "expected ok, got {result:?}");
        assert_eq!(after.as_deref(), Some("one\nTWO\nthree\n"));
    }

    #[test]
    fn edit_file_errors_on_zero_matches() {
        let path = home_join("zero");
        let original = "one\ntwo\n";
        fs::write(&path, original).unwrap();

        let input = json!({"path": &path, "old_text": "nonexistent", "new_text": "X"}).to_string();
        let result = run_edit_file(&input);
        let after = fs::read_to_string(&path).ok();
        let _ = fs::remove_file(&path);

        let err = result.expect_err("expected not-found error");
        assert!(err.contains("not found"), "got: {err}");
        assert_eq!(after.as_deref(), Some(original));
    }

    #[test]
    fn edit_file_zero_match_reports_over_escaped_backslashes() {
        // Dogfood T2: old_text doubled the backslashes of a raw-string regex
        // (JSON-escaping confusion). The error must name the real cause, not
        // just say "not found".
        let path = home_join("nearmiss");
        let original = "fn pat() {\n    let re = r\"^\\s*fn\";\n}\n";
        fs::write(&path, original).unwrap();

        let input = json!({
            "path": &path,
            "old_text": "    let re = r\"^\\\\s*fn\";\n",
            "new_text": "    let re = r\"^\\\\s*struct\";\n"
        })
        .to_string();
        let result = run_edit_file(&input);
        let _ = fs::remove_file(&path);

        let err = result.expect_err("expected not-found error");
        assert!(err.contains("not found"), "got: {err}");
        assert!(err.contains("over-escapes backslashes"), "got: {err}");
    }

    #[test]
    fn schemas_lists_five_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 5);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "bash",
                "edit_file",
                "bash_background",
                "bash_status",
                "bash_tail",
            ]
        );
    }

    // ─── bash_background family ──────────────────────────────────────────

    #[test]
    fn bash_background_rejects_missing_command() {
        let err = run_bash_background("{}").unwrap_err();
        assert!(err.contains("missing 'command'"), "got: {err}");
    }

    #[test]
    fn bash_background_rejects_empty_command() {
        let err = run_bash_background(r#"{"command":""}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn bash_status_rejects_unknown_job() {
        let err = run_bash_status(r#"{"job_id":"bg_does_not_exist_999"}"#).unwrap_err();
        assert!(err.contains("no job"), "got: {err}");
    }

    #[test]
    fn bash_tail_rejects_unknown_job() {
        let err = run_bash_tail(r#"{"job_id":"bg_does_not_exist_999"}"#).unwrap_err();
        assert!(err.contains("no job"), "got: {err}");
    }

    #[test]
    fn bash_background_status_tail_round_trip() {
        // Spawn a tiny background command, wait for it to finish, then
        // check status + tail. Use platform-portable commands.
        // Jobs land under ~/.claudette/jobs — home-resolving, so hold the
        // env lock against parallel temp-home swaps.
        let _eg = crate::test_env_lock();
        #[cfg(target_os = "windows")]
        let cmd = r"Write-Output hello-bg; Write-Error world-err";
        #[cfg(not(target_os = "windows"))]
        let cmd = "echo hello-bg; echo world-err 1>&2";

        let spawn_out = run_bash_background(&json!({ "command": cmd }).to_string()).expect("spawn");
        let v: Value = serde_json::from_str(&spawn_out).unwrap();
        let job_id = v["job_id"].as_str().unwrap().to_string();
        assert!(job_id.starts_with("bg_"));
        assert!(v["pid"].as_u64().is_some());

        // Wait for the reaper thread to land .done. Poll up to ~5s.
        let (_, _, _, done_path) = job_paths(&job_id);
        for _ in 0..50 {
            if done_path.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let status_out =
            run_bash_status(&json!({ "job_id": &job_id }).to_string()).expect("status");
        let s: Value = serde_json::from_str(&status_out).unwrap();
        assert_eq!(s["state"], "exited", "status did not transition: {s}");
        assert!(s["runtime_ms"].as_i64().unwrap() >= 0);

        let tail_out =
            run_bash_tail(&json!({ "job_id": &job_id, "stream": "both", "lines": 50 }).to_string())
                .expect("tail");
        let t: Value = serde_json::from_str(&tail_out).unwrap();
        let stdout = t["stdout"].as_array().unwrap();
        let stderr = t["stderr"].as_array().unwrap();
        assert!(
            stdout
                .iter()
                .any(|l| l.as_str().unwrap_or("").contains("hello-bg")),
            "stdout missing hello-bg: {stdout:?}"
        );
        assert!(
            stderr
                .iter()
                .any(|l| l.as_str().unwrap_or("").contains("world-err")),
            "stderr missing world-err: {stderr:?}"
        );

        // Cleanup — best-effort, don't fail the test if the OS hasn't
        // released the file handles yet.
        let (meta_p, out_p, err_p, done_p) = job_paths(&job_id);
        let _ = fs::remove_file(&meta_p);
        let _ = fs::remove_file(&out_p);
        let _ = fs::remove_file(&err_p);
        let _ = fs::remove_file(&done_p);
    }
}
