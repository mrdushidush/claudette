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
//!   <id>.meta — JSON {job_id, pid, cmd, cwd, started_at}. `cmd` is redacted
//!               before it is written (roast 2026-06-30); the file is created
//!               0600 / icacls-tightened like the secret store.
//!   <id>.out  — captured stdout (0600). The child writes raw bytes; `bash_tail`
//!   <id>.err  — captured stderr (0600). redacts each line on the way back out.
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
                        "new_text": { "type": "string", "description": "Replacement text" },
                        "replace_all": { "type": "boolean", "description": "When true, replace EVERY occurrence of old_text (for an intentional rename-everywhere) and report the count. Default false: exactly one match is required, and more than one is refused as ambiguous (the safe default)." }
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

/// Wave 1.1 (roast 2026-06-21): refuse the raw-shell escape hatch under
/// `--offline`. `bash` / `bash_background` run an arbitrary command — an
/// UNGUARDABLE egress vector, since a curl/scp/ssh/python/nc denylist leaks by
/// construction — so the only honest air-gap posture is to refuse the whole
/// tool while offline rather than pretend a substring filter closes the hole.
/// The structured tools (edit_file, search, git_* locals) keep coding
/// offline-capable; the build/test runners (`run_tests` / `diagnostics`) are
/// refused too, for the same unguardable-egress reason — they execute arbitrary
/// build scripts / test code (roast 2026-06-30, H1; see
/// `quality::refuse_toolchain_under_offline`). Returns the uniform
/// `BLOCK_PREFIX` refusal so the air-gap proof (`tests/offline_egress.rs`)
/// recognises it. Called *before* input parsing so a refusal fires regardless
/// of arguments.
fn refuse_bash_under_offline(tool: &str) -> Result<(), String> {
    if crate::egress::is_offline() {
        return Err(format!(
            "{}: {tool} is disabled under offline mode — a raw shell command can reach the \
             network in ways the air-gap guard cannot inspect (curl/scp/ssh/python/nc/…). Use \
             the structured tools instead, or disable offline mode to run shell commands.",
            crate::egress::BLOCK_PREFIX
        ));
    }
    Ok(())
}

fn run_bash(input: &str) -> Result<String, String> {
    refuse_bash_under_offline("bash")?;
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
    destructive_git_guard(command, &cwd)?;
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

// ────── destructive-git guard ───────────────────────────────────────────
//
// Dogfood hardening: `git reset --hard` (and force checkout/switch) run while
// the working tree is dirty have silently wiped in-progress edits twice in the
// co-dev loop — the brain edits files, then runs a "refresh main" reset that
// discards them, and the re-apply lands incomplete. When the model's `bash`
// command is one of these ops AND the target tree has uncommitted *tracked*
// changes, refuse it and point at the non-destructive branch recipe. A clean
// tree, a non-repo, or CLAUDETTE_ALLOW_DESTRUCTIVE_GIT=1 all pass through.

/// Refuse a destructive git command that would discard uncommitted tracked work.
fn destructive_git_guard(command: &str, cwd: &Path) -> Result<(), String> {
    // Fail-SAFE: only a canonical truthy value bypasses the guard. A previous
    // `var_os(...).is_some()` check was fail-OPEN — `=0` / `=false` / `""` all
    // disabled the data-loss guard despite the docs saying "=1". (roast
    // 2026-06-30 Theme C)
    if crate::env_config::is_enabled("CLAUDETTE_ALLOW_DESTRUCTIVE_GIT") {
        return Ok(());
    }
    let Some((op, work_dir)) = scan_destructive_git(command) else {
        return Ok(());
    };
    // The op acts on `git -C <dir>` if given, else the shell cwd.
    let dir: PathBuf = match work_dir {
        Some(d) => {
            let p = PathBuf::from(&d);
            if p.is_absolute() {
                p
            } else {
                cwd.join(p)
            }
        }
        None => cwd.to_path_buf(),
    };
    let dirty = git_tracked_dirty(&dir);
    if dirty.is_empty() {
        // Clean tree, not a git repo, or git unavailable — nothing to lose.
        return Ok(());
    }
    let shown: Vec<&str> = dirty.iter().take(8).map(String::as_str).collect();
    let more = dirty.len().saturating_sub(shown.len());
    let more_note = if more > 0 {
        format!(", +{more} more")
    } else {
        String::new()
    };
    Err(format!(
        "Refusing `{op}`: {} file(s) have uncommitted changes it would permanently discard \
         ({}{}). Commit them first (`git add -A && git commit -m ...`) or stash \
         (`git stash`). To start a fresh branch off the latest main WITHOUT losing these \
         edits, run `git fetch origin && git checkout -b <branch> origin/main` — that \
         carries your changes onto the new branch. (Override only if you truly mean to \
         discard them: set CLAUDETTE_ALLOW_DESTRUCTIVE_GIT=1.)",
        dirty.len(),
        shown.join(", "),
        more_note,
    ))
}

/// Scan a (possibly chained) command line for a destructive git op, returning
/// the op label and any `-C <dir>` target. Splits on shell separators so
/// `git fetch && git reset --hard` is caught, and resolves the real git
/// *subcommand* so `git commit -m "reset --hard"` is NOT a match.
fn scan_destructive_git(command: &str) -> Option<(&'static str, Option<String>)> {
    let normalized = command
        .replace("&&", "\n")
        .replace("||", "\n")
        .replace(['|', '&', ';'], "\n");
    normalized.lines().find_map(segment_destructive_git)
}

fn segment_destructive_git(segment: &str) -> Option<(&'static str, Option<String>)> {
    let mut words = segment.split_whitespace();
    // The command word must actually be `git` (after any leading VAR=val env
    // prefixes), so `echo git reset --hard` / `sudo git ...` don't trigger.
    let mut cmd_word = words.next()?;
    while is_env_assignment(cmd_word) {
        cmd_word = words.next()?;
    }
    if cmd_word != "git" {
        return None;
    }
    // Skip git global options (some take a value); capture `-C <dir>`.
    let args: Vec<&str> = words.collect();
    let mut i = 0;
    let mut work_dir = None;
    while i < args.len() {
        match args[i] {
            "-C" => {
                work_dir = args.get(i + 1).map(|s| (*s).to_string());
                i += 2;
            }
            "--git-dir" | "--work-tree" | "--namespace" | "-c" => i += 2,
            w if w.starts_with("--") && w.contains('=') => i += 1,
            w if w.starts_with('-') => i += 1,
            _ => break, // first bare token is the subcommand
        }
    }
    let sub = *args.get(i)?;
    let flags = &args[i + 1..];
    let has = |f: &str| flags.contains(&f);
    let op = match sub {
        "reset" if has("--hard") => "git reset --hard",
        "checkout" if has("-f") || has("--force") => "git checkout --force",
        "switch" if has("-f") || has("--force") || has("--discard-changes") => "git switch --force",
        _ => return None,
    };
    Some((op, work_dir))
}

/// `VAR=value` shell env-assignment prefix (uppercase/digit/underscore key).
fn is_env_assignment(word: &str) -> bool {
    if word.starts_with('-') {
        return false;
    }
    match word.split_once('=') {
        Some((k, _)) => {
            !k.is_empty()
                && k.chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        }
        None => false,
    }
}

/// Tracked, uncommitted changes in `dir` (the modifications/deletions that
/// `reset --hard` / force-checkout would destroy). Untracked files are
/// excluded — reset --hard leaves those. Empty if `dir` isn't a git repo or
/// git is unavailable, so the guard never blocks outside a dirty repo.
fn git_tracked_dirty(dir: &Path) -> Vec<String> {
    let Ok(out) = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .current_dir(dir)
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        // Porcelain v1: two status chars + space + path.
        .map(|l| l.get(3..).unwrap_or(l).trim().to_string())
        .collect()
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
    let replace_all = v
        .get("replace_all")
        .and_then(Value::as_bool)
        .unwrap_or(false);

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
        _ if replace_all && !old_text.is_empty() => {} // rename-everywhere: replace all n
        n => {
            return Err(format!(
                "edit_file: old_text appears {n} times in {}. Supply a longer, unique old_text (include surrounding context) so the target is unambiguous.",
                path.display()
            ));
        }
    }

    let new_content = if replace_all {
        content.replace(old_text, new_text)
    } else {
        content.replacen(old_text, new_text, 1)
    };

    // No-op guard (dogfood 2026-06-13): old_text == new_text writes the file
    // unchanged but reports ok:true — a false success that spirals small brains
    // into re-sending the same edit (the display layer hides over-escaped
    // backslashes, so they cannot see the blocks are identical). Fail loudly.
    if new_content == content {
        return Err(format!(
            "edit_file: no change — 'old_text' and 'new_text' are identical, so \
             nothing was written to {}. Re-read the file to see its CURRENT \
             contents: what you intend to change may already be present. Do NOT \
             re-send this edit unchanged.",
            path.display()
        ));
    }

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
    if replace_all {
        result["replacements"] = json!(match_count);
    }

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
    refuse_bash_under_offline("bash_background")?;
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
    destructive_git_guard(command, &cwd)?;

    ensure_dir(&jobs_dir())?;
    let job_id = new_job_id();
    let (meta_path, out_path, err_path, done_path) = job_paths(&job_id);

    // 0600 on Unix / icacls-tightened on Windows (roast 2026-06-30): the
    // child writes raw stdout/stderr here — a leaked AWS key in `env` output or
    // a token in a build log would otherwise land under the default umask,
    // readable by co-tenant users. The `bash_tail` reader redacts on the way
    // back out; this closes the at-rest half.
    let out_file = crate::secrets::create_private_file(&out_path)
        .map_err(|e| format!("bash_background: open {} failed: {e}", out_path.display()))?;
    let err_file = crate::secrets::create_private_file(&err_path)
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
        // Redact before persisting (roast 2026-06-30): the command can carry a
        // PAT (`git push https://x-access-token:<PAT>@…`) and bash_status echoes
        // meta.cmd back to the model. Mask it at rest and on the way out.
        cmd: crate::redact::redact(command).into_owned(),
        cwd: cwd.display().to_string(),
        started_at: chrono::Local::now().to_rfc3339(),
    };
    // 0600 / icacls — the meta file holds the (now redacted) command and pid;
    // keep it owner-only like the secret store rather than umask-default.
    crate::secrets::write_secret_file(
        &meta_path,
        serde_json::to_string_pretty(&meta)
            .unwrap_or_default()
            .as_bytes(),
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
    // Redact each surfaced line (roast 2026-06-30): the child wrote raw
    // stdout/stderr to disk, so a token echoed by a build/log command would
    // otherwise reach the model (and any transcript) verbatim.
    all[start..]
        .iter()
        .map(|s| crate::redact::redact(s).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_file_redacts_surfaced_secrets() {
        // The child writes raw stdout to disk; bash_tail must redact what it
        // surfaces to the model so a leaked key/token never round-trips.
        let path = std::env::temp_dir().join(format!(
            "claudette-tailtest-{}-{}.out",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        std::fs::write(
            &path,
            "starting build\nexport AWS=AKIAIOSFODNN7EXAMPLE\n\
             remote https://ghp_ABCDEFGHIJKLMNOP0123456789@github.com/x/y\ndone\n",
        )
        .expect("write tmp out");
        let lines = tail_file(&path, 100);
        let _ = std::fs::remove_file(&path);
        let joined = lines.join("\n");
        assert!(
            !joined.contains("AKIAIOSFODNN7EXAMPLE"),
            "leaked aws key: {joined}"
        );
        assert!(
            !joined.contains("ghp_ABCDEFGHIJKLMNOP"),
            "leaked github token: {joined}"
        );
        assert!(joined.contains("<redacted:aws-key>"), "got: {joined}");
        assert!(joined.contains("<redacted:github-token>"), "got: {joined}");
        assert!(
            joined.contains("starting build") && joined.contains("done"),
            "non-secret lines must survive: {joined}"
        );
    }

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
    fn scan_flags_reset_hard_plain_and_chained() {
        assert_eq!(
            scan_destructive_git("git reset --hard origin/main"),
            Some(("git reset --hard", None))
        );
        // chained after a refresh — the real dogfood failure shape
        assert_eq!(
            scan_destructive_git(
                "git checkout main && git fetch origin && git reset --hard origin/main"
            ),
            Some(("git reset --hard", None))
        );
        assert_eq!(
            scan_destructive_git("git fetch origin ; git reset --hard"),
            Some(("git reset --hard", None))
        );
    }

    #[test]
    fn scan_flags_force_checkout_and_switch() {
        assert_eq!(
            scan_destructive_git("git checkout -f"),
            Some(("git checkout --force", None))
        );
        assert_eq!(
            scan_destructive_git("git checkout --force main"),
            Some(("git checkout --force", None))
        );
        assert_eq!(
            scan_destructive_git("git switch --discard-changes main"),
            Some(("git switch --force", None))
        );
    }

    #[test]
    fn scan_captures_dash_c_workdir() {
        assert_eq!(
            scan_destructive_git("git -C /repo reset --hard"),
            Some(("git reset --hard", Some("/repo".to_string())))
        );
    }

    #[test]
    fn scan_ignores_safe_and_quoted_commands() {
        // non-destructive variants
        assert_eq!(scan_destructive_git("git reset --soft HEAD~1"), None);
        assert_eq!(scan_destructive_git("git reset HEAD~1"), None);
        assert_eq!(
            scan_destructive_git("git checkout -b feat/x origin/main"),
            None
        );
        assert_eq!(scan_destructive_git("git status"), None);
        // subcommand is `commit`; --hard only appears inside the message
        assert_eq!(
            scan_destructive_git(r#"git commit -m "reset --hard fixed the bug""#),
            None
        );
        // git is not the command word
        assert_eq!(scan_destructive_git("echo git reset --hard"), None);
    }

    #[test]
    fn guard_refuses_reset_hard_on_dirty_tree_but_allows_clean() {
        // Hold the env lock so the sibling fail-safe test (which mutates
        // CLAUDETTE_ALLOW_DESTRUCTIVE_GIT) can't flip the guard mid-assertion.
        let _lock = crate::test_env_lock();
        // Skip if git is unavailable or the override is *enabled* in the env
        // (matches the guard's own fail-safe check — a falsey value no longer
        // bypasses the guard, so the test can still run).
        if crate::env_config::is_enabled("CLAUDETTE_ALLOW_DESTRUCTIVE_GIT") {
            return;
        }
        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !git_ok {
            return;
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("claudette-gitguard-{nanos}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);

        // Clean tree → guard allows the reset.
        assert!(git_tracked_dirty(&dir).is_empty());
        assert!(destructive_git_guard("git reset --hard", &dir).is_ok());

        // Dirty tree → guard refuses and names the file.
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        assert!(git_tracked_dirty(&dir).iter().any(|f| f == "a.txt"));
        let err = destructive_git_guard("git reset --hard origin/main", &dir).unwrap_err();
        assert!(err.contains("Refusing"), "got: {err}");
        assert!(err.contains("a.txt"), "should name the file: {err}");
        assert!(
            err.contains("checkout -b"),
            "should suggest the safe recipe: {err}"
        );

        // A non-destructive command on the same dirty tree still passes.
        assert!(destructive_git_guard("git status", &dir).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn destructive_git_guard_is_fail_safe_not_fail_open() {
        // Regression for roast 2026-06-30 Theme C: the guard used to check
        // `var_os(...).is_some()`, so ANY value — `=0`, `=false`, `""` —
        // disabled the data-loss guard (fail-OPEN) despite the docs saying
        // "=1". It must now bypass ONLY on a canonical truthy value.
        let _lock = crate::test_env_lock();
        let key = "CLAUDETTE_ALLOW_DESTRUCTIVE_GIT";
        let prev = std::env::var(key).ok();

        let git_ok = std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if !git_ok {
            return;
        }

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("claudette-gitguard-failsafe-{nanos}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&dir)
                .output()
                .unwrap()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-qm", "init"]);
        // Make the tree dirty so the guard has something to protect.
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        assert!(git_tracked_dirty(&dir).iter().any(|f| f == "a.txt"));

        // Falsey / empty values must NOT bypass — the guard stays active.
        for v in ["0", "false", "off", ""] {
            std::env::set_var(key, v);
            assert!(
                destructive_git_guard("git reset --hard origin/main", &dir).is_err(),
                "value '{v}' must NOT bypass the guard (fail-open regression)"
            );
        }

        // Canonical truthy values (case-insensitive) bypass as documented.
        for v in ["1", "true", "TRUE", "yes", "on"] {
            std::env::set_var(key, v);
            assert!(
                destructive_git_guard("git reset --hard origin/main", &dir).is_ok(),
                "value '{v}' should bypass the guard"
            );
        }

        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
        let _ = std::fs::remove_dir_all(&dir);
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
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-HOME swaps
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
    fn edit_file_replace_all_replaces_every_occurrence() {
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-HOME swaps
        let path = home_join("replace_all");
        fs::write(&path, "foo / foo / foo\n").unwrap();

        let input =
            json!({"path": &path, "old_text": "foo", "new_text": "bar", "replace_all": true})
                .to_string();
        let result = run_edit_file(&input);
        let after = fs::read_to_string(&path).ok();
        let _ = fs::remove_file(&path);

        assert!(result.is_ok(), "expected ok, got {result:?}");
        let out = result.unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["replacements"], json!(3), "got: {out}");
        assert_eq!(after.as_deref(), Some("bar / bar / bar\n"));
    }

    #[test]
    fn edit_file_without_replace_all_still_refuses_multiple_matches() {
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-HOME swaps
        let path = home_join("replace_all_no_default");
        fs::write(&path, "foo / foo / foo\n").unwrap();

        // Same input, no replace_all → the ambiguity guard still fires.
        let input = json!({"path": &path, "old_text": "foo", "new_text": "bar"}).to_string();
        let result = run_edit_file(&input);
        let after = fs::read_to_string(&path).ok();
        let _ = fs::remove_file(&path);

        let err = result.expect_err("expected ambiguity error");
        assert!(err.contains("appears 3 times"), "got: {err}");
        assert_eq!(after.as_deref(), Some("foo / foo / foo\n"));
    }

    #[test]
    fn edit_file_replace_all_still_fires_noop_guard() {
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-HOME swaps
        let path = home_join("replace_all_noop");
        fs::write(&path, "foo foo\n").unwrap();

        // old_text == new_text under replace_all is still a loud no-op, not ok:true.
        let input =
            json!({"path": &path, "old_text": "foo", "new_text": "foo", "replace_all": true})
                .to_string();
        let result = run_edit_file(&input);
        let after_disk = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);

        let err = result.expect_err("identical old/new must be a no-op error");
        assert!(err.contains("no change"), "got: {err}");
        assert_eq!(
            after_disk, "foo foo\n",
            "file must not be modified by a no-op"
        );
    }

    #[test]
    fn edit_file_replaces_unique_match() {
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-HOME swaps
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
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-HOME swaps
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
    fn edit_file_identical_old_new_is_a_loud_no_op() {
        // Dogfood 2026-06-13: old_text == new_text writes nothing but reported
        // ok:true. It must now FAIL with a no-op error and leave the file alone.
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-HOME swaps
        let path = home_join("noop");
        let original = "alpha\nbeta\ngamma\n";
        fs::write(&path, original).unwrap();

        let input = json!({"path": &path, "old_text": "beta\n", "new_text": "beta\n"}).to_string();
        let result = run_edit_file(&input);
        let after_disk = fs::read_to_string(&path).unwrap();
        let _ = fs::remove_file(&path);

        let err = result.expect_err("identical old/new must be a no-op error");
        assert!(err.contains("no change"), "got: {err}");
        assert_eq!(after_disk, original, "file must not be modified by a no-op");
    }

    #[test]
    fn edit_file_zero_match_reports_over_escaped_backslashes() {
        // Dogfood T2: old_text doubled the backslashes of a raw-string regex
        // (JSON-escaping confusion). The error must name the real cause, not
        // just say "not found".
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-HOME swaps
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
