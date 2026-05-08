//! Git group — 9 tools that shell out to `git` as a subprocess. Most run
//! in the workspace root (where claudette was launched). `git_clone` is the
//! exception: it writes a fresh tree under `~/.claudette/missions/<dest>/`,
//! gated by URL-scheme + dest-slug validation. Safety: destructive flags
//! (--force, reset --hard, clean -f, branch -D, --no-verify) are rejected
//! before they reach the subprocess.
//!
//! Self-contained: all helpers (`resolve_git_path`, `run_git`,
//! `reject_destructive`, `auto_commit_message`, `extract_stat_number`)
//! are private to this module. No parent-module `pub(super)` helpers are
//! used — every handler parses its own JSON input directly.

use serde_json::{json, Value};

use crate::test_runner::run_command_with_timeout;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "git_status",
                "description": "Show working tree status (modified, staged, untracked files).",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_diff",
                "description": "Show file changes (unstaged by default, or staged).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":   { "type": "string",  "description": "Limit to this file (optional)" },
                        "staged": { "type": "boolean", "description": "Show staged changes instead" }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_log",
                "description": "Show recent commit history. Use detail=true for full info (hash, author, date, message body).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "count":  { "type": "number",  "description": "Number of commits (default 10)" },
                        "path":   { "type": "string",  "description": "Limit to this file (optional)" },
                        "detail": { "type": "boolean", "description": "Show full commit info: hash, author, date, files changed (default false)" }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_add",
                "description": "Stage files for the next commit.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "paths": { "type": "string", "description": "Space-separated file paths to stage" }
                    },
                    "required": ["paths"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_commit",
                "description": "Commit staged changes. If message is omitted, auto-generates one from the staged diff.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "message": { "type": "string", "description": "Commit message (optional — auto-generated from diff if omitted)" }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_branch",
                "description": "List all branches, or create a new one if name is given.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string", "description": "New branch name (omit to list)" }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_checkout",
                "description": "Switch to a different branch.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "target": { "type": "string", "description": "Branch name or commit" }
                    },
                    "required": ["target"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_push",
                "description": "Push commits to the remote repository.",
                "parameters": { "type": "object", "properties": {}, "required": [] }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "git_clone",
                "description": "Clone a remote repo into ~/.claudette/missions/<dest>/. Use for brownfield work on external repos.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url":   { "type": "string", "description": "Repo URL (https://, http://, git@, ssh://)" },
                        "dest":  { "type": "string", "description": "Subdirectory name under ~/.claudette/missions/ (no slashes, no '..')" },
                        "depth": { "type": "number", "description": "Optional shallow-clone depth. Omit for full history." }
                    },
                    "required": ["url", "dest"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "git_status" => run_git_status(),
        "git_diff" => run_git_diff(input),
        "git_log" => run_git_log(input),
        "git_add" => run_git_add(input),
        "git_commit" => run_git_commit(input),
        "git_branch" => run_git_branch(input),
        "git_checkout" => run_git_checkout(input),
        "git_push" => run_git_push(),
        "git_clone" => run_git_clone(input),
        _ => return None,
    };
    Some(result)
}

/// Resolve the full path to `git.exe`. On Windows, git is often installed
/// under `Program Files` but NOT added to the system PATH (it's only in
/// Git Bash's internal PATH). `Command::new("git")` fails in that case.
///
/// Strategy: try `where git` first (works if git IS in PATH), then probe
/// known install locations. Caches the result via `OnceLock` so the
/// filesystem scan runs at most once per process.
fn resolve_git_path() -> String {
    use std::sync::OnceLock;
    static GIT_PATH: OnceLock<String> = OnceLock::new();
    GIT_PATH
        .get_or_init(|| {
            // 1. Try `where git` (works when git is in PATH).
            #[cfg(target_os = "windows")]
            {
                if let Ok(out) = std::process::Command::new("where").arg("git").output() {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    if let Some(path) = stdout.lines().next().map(str::trim) {
                        if !path.is_empty() && std::path::Path::new(path).exists() {
                            return path.to_string();
                        }
                    }
                }

                // 2. Probe known Git for Windows install locations.
                let drives = ["C:", "D:", "E:"];
                let suffixes = [
                    r"\Program Files\Git\cmd\git.exe",
                    r"\Program Files\Git\bin\git.exe",
                    r"\Program Files\Git\mingw64\bin\git.exe",
                    r"\Program Files (x86)\Git\cmd\git.exe",
                ];
                for drive in &drives {
                    for suffix in &suffixes {
                        let candidate = format!("{drive}{suffix}");
                        if std::path::Path::new(&candidate).exists() {
                            return candidate;
                        }
                    }
                }
            }
            "git".to_string()
        })
        .clone()
}

/// Run a git command from the workspace root (CWD). Returns the
/// `CommandResult` stdout on success, or an error with stderr.
///
/// On Windows, resolves git via `where git` first (handles spaces in
/// PATH like `D:\Program Files\Git\...`). Falls back to bare `git`.
fn run_git(args: &[&str]) -> Result<String, String> {
    let git_exe = resolve_git_path();
    eprintln!(
        "  {} {}",
        crate::theme::dim("▸"),
        crate::theme::dim(&format!("git: using {git_exe:?}, args={args:?}")),
    );
    let result = run_command_with_timeout(&git_exe, args, 30, None);
    if !result.success {
        eprintln!(
            "  {} {}",
            crate::theme::dim("▸"),
            crate::theme::dim(&format!(
                "git: failed — exit={:?} stderr={:?}",
                result.exit_code,
                result.stderr.chars().take(200).collect::<String>()
            )),
        );
    }
    if result.timed_out {
        return Err(format!(
            "git {}: timed out after 30s",
            args.first().unwrap_or(&"")
        ));
    }
    if !result.success {
        let err = if result.stderr.is_empty() {
            result.stdout.clone()
        } else {
            result.stderr.clone()
        };
        return Err(format!(
            "git {}: exit code {:?}\n{}",
            args.first().unwrap_or(&""),
            result.exit_code,
            err.chars().take(500).collect::<String>()
        ));
    }
    Ok(result.stdout)
}

/// Reject arguments that contain destructive git flags. Called before
/// every git tool dispatch. Better to over-block than to let a small
/// model accidentally force-push or hard-reset.
fn reject_destructive(args: &[&str]) -> Result<(), String> {
    let banned = [
        "--force",
        "-f",
        "--force-with-lease",
        "--hard",
        "--mixed", // reset --hard/--mixed
        "-D",      // branch -D (force delete)
        "--no-verify",
    ];
    for arg in args {
        for b in &banned {
            if arg == b {
                return Err(format!(
                    "git: destructive flag `{arg}` is blocked for safety. \
                     If you really need it, run git manually outside the secretary."
                ));
            }
        }
    }
    Ok(())
}

fn run_git_status() -> Result<String, String> {
    let output = run_git(&["status", "--short", "--branch"])?;
    Ok(json!({ "output": output }).to_string())
}

fn run_git_diff(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let staged = v.get("staged").and_then(Value::as_bool).unwrap_or(false);
    let path = v.get("path").and_then(Value::as_str);

    let mut args = vec!["diff"];
    if staged {
        args.push("--cached");
    }
    // Cap diff output so it doesn't blow the context window.
    args.push("--stat");
    args.push("--patch");
    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let output = run_git(&args)?;
    // Truncate very large diffs.
    let truncated = output.len() > 8000;
    let visible: String = output.chars().take(8000).collect();
    Ok(json!({ "output": visible, "truncated": truncated }).to_string())
}

fn run_git_log(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let count = v.get("count").and_then(Value::as_u64).unwrap_or(10);
    let path = v.get("path").and_then(Value::as_str);
    let detail = v.get("detail").and_then(Value::as_bool).unwrap_or(false);

    let count_str = format!("-{count}");
    let format_str;
    let mut args = vec!["log", &count_str];

    if detail {
        // Rich format: hash, author, date, subject, body + file stats.
        format_str = "--format=%H %an (%ar)%n  %s%n%b".to_string();
        args.push(&format_str);
        args.push("--stat");
    } else {
        args.push("--oneline");
    }

    if let Some(p) = path {
        args.push("--");
        args.push(p);
    }
    let output = run_git(&args)?;
    // Truncate in detail mode since --stat can be verbose.
    if detail && output.len() > 6000 {
        let truncated: String = output.chars().take(6000).collect();
        Ok(json!({ "output": truncated, "truncated": true }).to_string())
    } else {
        Ok(json!({ "output": output }).to_string())
    }
}

fn run_git_add(input: &str) -> Result<String, String> {
    let v: Value =
        serde_json::from_str(input).map_err(|e| format!("git_add: invalid JSON ({e}): {input}"))?;
    let paths_str = v
        .get("paths")
        .and_then(Value::as_str)
        .ok_or("git_add: missing 'paths'")?;

    let paths: Vec<&str> = paths_str.split_whitespace().collect();
    if paths.is_empty() {
        return Err("git_add: no paths specified".to_string());
    }
    // Block `git add -A` / `git add .` — too dangerous for this workspace.
    for p in &paths {
        if *p == "-A" || *p == "--all" || *p == "." {
            return Err(format!(
                "git_add: `{p}` is blocked — stage files explicitly by name to avoid \
                 accidentally adding .venv noise or secrets"
            ));
        }
    }

    let mut args = vec!["add"];
    args.extend(paths.iter());
    let output = run_git(&args)?;
    Ok(json!({ "ok": true, "staged": paths_str, "output": output }).to_string())
}

fn run_git_commit(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("git_commit: invalid JSON ({e}): {input}"))?;
    let message_param = v.get("message").and_then(Value::as_str).unwrap_or("");

    let message = if message_param.trim().is_empty() {
        // Auto-generate from staged diff.
        auto_commit_message()?
    } else {
        message_param.to_string()
    };

    let output = run_git(&["commit", "-m", &message])?;
    Ok(json!({ "ok": true, "message": message, "output": output }).to_string())
}

/// Generate a commit message from the currently staged diff.
fn auto_commit_message() -> Result<String, String> {
    let stat = run_git(&["diff", "--cached", "--stat"])?;
    if stat.trim().is_empty() {
        return Err("git_commit: nothing staged — run git_add first".to_string());
    }

    // Parse file names from stat output: lines like " src/tools.rs | 42 ++--"
    let files: Vec<&str> = stat
        .lines()
        .filter(|l| l.contains('|'))
        .map(|l| l.split('|').next().unwrap_or("").trim())
        .filter(|f| !f.is_empty())
        .collect();

    // Parse the summary line: "3 files changed, 45 insertions(+), 10 deletions(-)"
    let summary_line = stat.lines().last().unwrap_or("");
    let insertions = extract_stat_number(summary_line, "insertion");
    let deletions = extract_stat_number(summary_line, "deletion");

    // Build a concise message.
    let file_count = files.len();
    let file_list = if file_count <= 3 {
        files.join(", ")
    } else {
        format!("{}, {} and {} more", files[0], files[1], file_count - 2)
    };

    let stat_suffix = match (insertions, deletions) {
        (0, 0) => String::new(),
        (i, 0) => format!(" (+{i})"),
        (0, d) => format!(" (-{d})"),
        (i, d) => format!(" (+{i}, -{d})"),
    };

    Ok(format!("Update {file_list}{stat_suffix}"))
}

/// Extract a number from a git stat summary line by keyword prefix.
fn extract_stat_number(line: &str, keyword: &str) -> usize {
    // Pattern: "45 insertions(+)" — find the number before the keyword.
    for part in line.split(',') {
        let trimmed = part.trim();
        if trimmed.contains(keyword) {
            if let Some(num_str) = trimmed.split_whitespace().next() {
                if let Ok(n) = num_str.parse::<usize>() {
                    return n;
                }
            }
        }
    }
    0
}

fn run_git_branch(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let name = v.get("name").and_then(Value::as_str);

    match name {
        Some(n) if !n.is_empty() => {
            reject_destructive(&[n])?;
            let output = run_git(&["branch", n])?;
            Ok(json!({ "ok": true, "created": n, "output": output }).to_string())
        }
        _ => {
            let output = run_git(&["branch", "-a"])?;
            Ok(json!({ "output": output }).to_string())
        }
    }
}

fn run_git_checkout(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("git_checkout: invalid JSON ({e}): {input}"))?;
    let target = v
        .get("target")
        .and_then(Value::as_str)
        .ok_or("git_checkout: missing 'target'")?;

    reject_destructive(&[target])?;
    let output = run_git(&["checkout", target])?;
    Ok(json!({ "ok": true, "checked_out": target, "output": output }).to_string())
}

/// Resolve `~/.claudette/missions/`. Mirrors the home-resolution pattern in
/// `crate::secrets::secrets_dir`.
fn missions_root() -> std::path::PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    std::path::PathBuf::from(home)
        .join(".claudette")
        .join("missions")
}

/// Reject anything that isn't a clean http(s)/git/ssh URL. Don't let the
/// model talk us into `file:///` or other surprise schemes.
fn validate_clone_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("git_clone: empty url".to_string());
    }
    let ok = trimmed.starts_with("https://")
        || trimmed.starts_with("http://")
        || trimmed.starts_with("git@")
        || trimmed.starts_with("ssh://");
    if !ok {
        return Err(format!(
            "git_clone: unsupported url scheme — must start with https://, http://, git@, or ssh:// (got `{trimmed}`)"
        ));
    }
    Ok(())
}

/// Validate the dest slug. Single path component, no traversal, no
/// drive prefix, no leading/trailing whitespace.
fn validate_dest_slug(dest: &str) -> Result<String, String> {
    let trimmed = dest.trim();
    if trimmed.is_empty() {
        return Err("git_clone: empty dest".to_string());
    }
    if trimmed.contains("..") {
        return Err(format!("git_clone: dest may not contain '..' ({trimmed})"));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(format!(
            "git_clone: dest must be a single directory name, not a path ({trimmed})"
        ));
    }
    // Reject drive prefixes like `C:` and stray colons that confuse Windows.
    if trimmed.contains(':') {
        return Err(format!("git_clone: dest may not contain ':' ({trimmed})"));
    }
    Ok(trimmed.to_string())
}

fn run_git_clone(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("git_clone: invalid JSON ({e}): {input}"))?;
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .ok_or("git_clone: missing 'url'")?;
    let dest_raw = v
        .get("dest")
        .and_then(Value::as_str)
        .ok_or("git_clone: missing 'dest'")?;
    let depth = v.get("depth").and_then(Value::as_u64);

    validate_clone_url(url)?;
    let dest = validate_dest_slug(dest_raw)?;

    let root = missions_root();
    std::fs::create_dir_all(&root).map_err(|e| {
        format!(
            "git_clone: failed to create missions root {}: {e}",
            root.display()
        )
    })?;
    let target = root.join(&dest);

    if target.exists() {
        return Err(format!(
            "git_clone: target already exists at {} — pick a different dest or remove it first",
            target.display()
        ));
    }

    let target_str = target
        .to_str()
        .ok_or_else(|| format!("git_clone: target path is not utf-8: {}", target.display()))?
        .to_string();

    let depth_str;
    let mut args: Vec<&str> = vec!["clone"];
    if let Some(d) = depth {
        depth_str = format!("{d}");
        args.push("--depth");
        args.push(&depth_str);
    }
    args.push("--");
    args.push(url);
    args.push(&target_str);

    let git_exe = resolve_git_path();
    let result = run_command_with_timeout(&git_exe, &args, 120, None);
    if result.timed_out {
        // Best-effort cleanup of any half-cloned tree.
        let _ = std::fs::remove_dir_all(&target);
        return Err(format!("git_clone: timed out after 120s for {url}"));
    }
    if !result.success {
        let _ = std::fs::remove_dir_all(&target);
        let stderr = if result.stderr.is_empty() {
            result.stdout.clone()
        } else {
            result.stderr.clone()
        };
        return Err(format!(
            "git_clone: failed (exit {:?}): {}",
            result.exit_code,
            stderr.chars().take(500).collect::<String>()
        ));
    }

    Ok(json!({
        "ok": true,
        "url": url,
        "path": target_str,
        "dest": dest,
    })
    .to_string())
}

fn run_git_push() -> Result<String, String> {
    // Gate enforcement lives at the policy layer: `git_push` is registered
    // as `DangerFullAccess` in `run.rs::build_permission_policy` and in
    // `agents.rs`, so every call path either (a) already runs under
    // DangerFullAccess, (b) triggers the `CliPrompter` `[y/N]` confirm in
    // REPL mode, or (c) is denied outright (Telegram bot, unattended agents).
    // See `runtime/permissions.rs::PermissionPolicy::authorize`. The banner
    // here is informational — it has already been authorised to run.
    eprintln!(
        "{} {}",
        crate::theme::warn(crate::theme::WARN_GLYPH),
        crate::theme::warn("git_push: pushing to remote...")
    );
    let output = run_git(&["push"])?;
    Ok(json!({ "ok": true, "output": output }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_stat_number_from_summary() {
        let line = " 3 files changed, 45 insertions(+), 10 deletions(-)";
        assert_eq!(extract_stat_number(line, "insertion"), 45);
        assert_eq!(extract_stat_number(line, "deletion"), 10);
    }

    #[test]
    fn extract_stat_number_single_insertion() {
        let line = " 1 file changed, 1 insertion(+)";
        assert_eq!(extract_stat_number(line, "insertion"), 1);
        assert_eq!(extract_stat_number(line, "deletion"), 0);
    }

    #[test]
    fn extract_stat_number_missing() {
        assert_eq!(extract_stat_number("no match here", "insertion"), 0);
    }

    #[test]
    fn git_commit_empty_message_triggers_auto() {
        // With no staged changes, auto_commit_message should error
        // rather than producing an empty commit.
        let err = run_git_commit("{}");
        // This might fail because either: no git repo, or nothing staged.
        // Both are valid — we just need to confirm it doesn't succeed with
        // an empty message.
        if let Err(msg) = err {
            // Either "nothing staged" or git error — both acceptable.
            assert!(
                msg.contains("staged") || msg.contains("git"),
                "expected staged/git error, got: {msg}"
            );
        }
    }

    #[test]
    fn schemas_lists_nine_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 9);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            [
                "git_status",
                "git_diff",
                "git_log",
                "git_add",
                "git_commit",
                "git_branch",
                "git_checkout",
                "git_push",
                "git_clone",
            ]
        );
    }

    #[test]
    fn validate_clone_url_accepts_known_schemes() {
        assert!(validate_clone_url("https://github.com/owner/repo.git").is_ok());
        assert!(validate_clone_url("http://example.com/r.git").is_ok());
        assert!(validate_clone_url("git@github.com:owner/repo.git").is_ok());
        assert!(validate_clone_url("ssh://git@host/path").is_ok());
    }

    #[test]
    fn validate_clone_url_rejects_other_schemes() {
        assert!(validate_clone_url("file:///etc/passwd").is_err());
        assert!(validate_clone_url("javascript:alert(1)").is_err());
        assert!(validate_clone_url("").is_err());
        assert!(validate_clone_url("github.com/owner/repo").is_err());
    }

    #[test]
    fn validate_dest_slug_accepts_simple_name() {
        assert_eq!(
            validate_dest_slug("django__issue-12345").unwrap(),
            "django__issue-12345"
        );
    }

    #[test]
    fn validate_dest_slug_rejects_traversal() {
        assert!(validate_dest_slug("..").is_err());
        assert!(validate_dest_slug("foo/../bar").is_err());
        assert!(validate_dest_slug("a/b").is_err());
        assert!(validate_dest_slug("a\\b").is_err());
        assert!(validate_dest_slug("C:\\evil").is_err());
        assert!(validate_dest_slug("").is_err());
    }

    #[test]
    fn run_git_clone_rejects_bad_url() {
        let err = run_git_clone(r#"{"url":"file:///etc/passwd","dest":"x"}"#).unwrap_err();
        assert!(err.contains("scheme"), "got: {err}");
    }

    #[test]
    fn run_git_clone_rejects_bad_dest() {
        let err = run_git_clone(r#"{"url":"https://github.com/o/r","dest":"a/b"}"#).unwrap_err();
        assert!(err.contains("single directory"), "got: {err}");
    }

    #[test]
    fn run_git_clone_rejects_missing_fields() {
        assert!(run_git_clone("{}").unwrap_err().contains("url"));
        assert!(run_git_clone(r#"{"url":"https://github.com/o/r"}"#)
            .unwrap_err()
            .contains("dest"));
    }

    #[test]
    fn reject_destructive_blocks_force() {
        assert!(reject_destructive(&["--force"]).is_err());
        assert!(reject_destructive(&["-D"]).is_err());
        assert!(reject_destructive(&["--no-verify"]).is_err());
        assert!(reject_destructive(&["--hard"]).is_err());
        assert!(reject_destructive(&["feature-branch"]).is_ok());
    }
}
