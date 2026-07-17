//! File ops group — 3 tools (read_file, write_file, list_dir).
//!
//! Sandboxing policy (set by the `validate_read_path` / `validate_write_path`
//! helpers in the parent module):
//!   - read_file / list_dir: allowed anywhere under the user's $HOME.
//!   - write_file: allowed ONLY under ~/.claudette/files/ (the scratch dir),
//!     the active mission tree, or an explicit `CLAUDETTE_WORKSPACE` project.
//!
//! write_file writes any text file directly — source code, config, data, or
//! markup. (Historically it refused code and routed it to a `generate_code`
//! coder sidecar; that sidecar was retired, so the brain writes code itself.)

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::{
    ensure_dir, file_url_for, files_dir, validate_read_path, validate_write_path, MAX_FILE_BYTES,
};

const MAX_LIST_ENTRIES: usize = 200;

/// Default number of lines `read_file` returns when the caller passes no
/// explicit `limit`. Whole-file reads of large files (e.g. run.rs ~2k lines)
/// blow a small local model's context window and, re-issued in a search
/// spiral, drive multi-minute hangs (observed on qwen3.6-35b q3 @ 32k: a
/// "where is X configured" locate read the same 2k-line file three times and
/// timed out). Capping the default — with a clear "use offset/limit or
/// grep_search" notice — keeps each read cheap and nudges targeted
/// navigation, mirroring Claude Code's windowed Read. Override via
/// `CLAUDETTE_READ_DEFAULT_LINES`.
const DEFAULT_READ_LINES: usize = 400;

fn read_default_line_cap() -> usize {
    std::env::var("CLAUDETTE_READ_DEFAULT_LINES")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_READ_LINES)
}

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a text file (max 100 KB). Returns up to 400 lines by default; for a larger file pass `offset`/`limit` to page through it (or `tail` for the last N lines), or use grep_search to jump straight to the line you need. Do not re-read the same range.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":   { "type": "string", "description": "File path (absolute, ~/, or relative to the workspace)" },
                        "offset": { "type": "integer", "description": "1-based line number to start at (default: start of file)" },
                        "limit":  { "type": "integer", "description": "Max lines to return (default: 400)" },
                        "tail":   { "type": "integer", "description": "Return only the LAST N lines (e.g. the end of a log). Mutually exclusive with offset." }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write a file — source code, config, data, or markup — to a new path. Writes the content in one pass. To modify an existing file, use edit_file instead of rewriting it whole.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path":    { "type": "string", "description": "Filename or path under the sandbox" },
                        "content": { "type": "string", "description": "Text content to write" }
                    },
                    "required": ["path", "content"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "list_dir",
                "description": "List files and folders in a directory under the user's home.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Directory path (absolute or ~/)" }
                    },
                    "required": ["path"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "read_file" => run_read_file(input),
        "write_file" => run_write_file(input),
        "list_dir" => run_list_dir(input),
        _ => return None,
    };
    Some(result)
}

fn run_read_file(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("read_file: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("read_file: missing 'path'")?;
    // 1-based start line; 0 or absent = start of file.
    let offset = v.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize;
    // Explicit line cap; absent = the default windowed cap.
    let explicit_limit = v.get("limit").and_then(Value::as_u64).map(|n| n as usize);

    // Optional tail window: the last N lines. Mutually exclusive with offset —
    // they specify opposite ends, so asking for both is a contradiction.
    let tail = v.get("tail").and_then(Value::as_u64).map(|n| n as usize);
    if tail.is_some() && offset > 0 {
        return Err(
            "read_file: 'tail' and 'offset' are mutually exclusive — pass one or the other"
                .to_string(),
        );
    }

    let path = validate_read_path(path_str)?;

    let metadata = fs::metadata(&path).map_err(|e| {
        // Verbose error so the brain stops papering over silent
        // missing-file outcomes (F5). Include the resolved absolute path
        // and the original user-supplied form so the brain can correct
        // its next call instead of hallucinating success.
        format!(
            "read_file: stat {} failed: {e}. (input path: {path_str}; \
             relative paths resolve against the active mission cwd or the \
             process cwd, with CLAUDETTE_WORKSPACE roots as a fallback.)",
            path.display()
        )
    })?;
    if metadata.is_dir() {
        return Err(format!(
            "read_file: {} is a directory; use list_dir instead",
            path.display()
        ));
    }
    let size = metadata.len();
    if size > MAX_FILE_BYTES as u64 {
        return Err(format!(
            "read_file: {} is {size} bytes, exceeds {MAX_FILE_BYTES}-byte limit",
            path.display()
        ));
    }

    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("read_file: read {} failed: {e}", path.display()))?;

    let lines: Vec<&str> = raw.lines().collect();
    let total = lines.len();
    let (start, end) = if let Some(n) = tail {
        // Last `n` lines (or the whole file when it has fewer than n).
        (total.saturating_sub(n), total)
    } else {
        let start = offset.saturating_sub(1).min(total);
        let cap = explicit_limit.unwrap_or_else(read_default_line_cap);
        let end = start.saturating_add(cap).min(total);
        (start, end)
    };
    let mut content = lines[start..end].join("\n");
    // Keep a trailing newline for a whole-small-file read so callers see the
    // file exactly as on disk.
    if end == total && raw.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }

    let truncated = end < total;
    if truncated {
        use std::fmt::Write;
        let _ = write!(
            content,
            "\n\n[read_file: showed lines {}-{} of {}. The file continues — \
             re-read with offset={} to page on, or use grep_search to jump \
             straight to a symbol. Do NOT re-read the same range.]",
            start + 1,
            end,
            total,
            end + 1,
        );
    }

    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": size,
        "lines_shown": format!("{}-{}", start + 1, end),
        "total_lines": total,
        "truncated": truncated,
        "content": content,
    })
    .to_string())
}

fn run_write_file(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("write_file: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("write_file: missing 'path'")?;
    let content = v
        .get("content")
        .and_then(Value::as_str)
        .ok_or("write_file: missing 'content'")?;

    if content.len() > MAX_FILE_BYTES {
        return Err(format!(
            "write_file: content is {} bytes, exceeds {MAX_FILE_BYTES}-byte limit",
            content.len()
        ));
    }

    // Bare relative paths get resolved under either the active mission
    // tree (T2) or the scratch sandbox, NOT against the process CWD.
    // Reasoning: the model says "save it to dolphins-post.txt" and
    // expects it to land somewhere reasonable. Pre-T2 we rooted bare
    // relative paths under ~/.claudette/files/. T2 keeps that fallback
    // but routes to the mission tree when a brownfield mission is
    // active — matching the brain's likely intent ("save README.md"
    // means *the project's* README, not a copy in scratch).
    // Absolute and ~/-prefixed paths still flow through validate_write_path
    // unchanged so the user can still explicitly target a sub-folder.
    let resolved_input = if Path::new(path_str).is_absolute()
        || path_str.starts_with("~/")
        || path_str.starts_with("~\\")
    {
        path_str.to_string()
    } else {
        // Bare relative path. Resolve against, in priority order: the active
        // mission tree → the user's explicit workspace CWD (daily-driver:
        // "save README.md here" means the project) → the scratch sandbox
        // (pure personal-assistant default when no workspace is set).
        let base = if crate::missions::active_mission().is_some() {
            crate::missions::active_cwd()
        } else if let Some(ws_cwd) = super::workspace_cwd() {
            ws_cwd
        } else {
            files_dir()
        };
        base.join(path_str).display().to_string()
    };
    let path = validate_write_path(&resolved_input)?;

    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    // Pre-image: an existing target is about to be truncated — snapshot it
    // to ~/.claudette/trash/ first so `/undo` can restore it. New files
    // have nothing to preserve. Fail-closed: no snapshot, no overwrite
    // (recoverability is the feature; a silent truncate was the data-loss
    // path the roast flagged).
    if path.exists() {
        crate::transcript::snapshot_to_trash(&path).map_err(|e| {
            format!(
                "write_file: pre-image snapshot failed, refusing to overwrite {}: {e}",
                path.display()
            )
        })?;
    }
    fs::write(&path, content)
        .map_err(|e| format!("write_file: write {} failed: {e}", path.display()))?;

    let result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "file_url": file_url_for(&path),
        "bytes": content.len(),
    });

    Ok(result.to_string())
}

fn run_list_dir(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("list_dir: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("list_dir: missing 'path'")?;

    let path = validate_read_path(path_str)?;

    let metadata = fs::metadata(&path)
        .map_err(|e| format!("list_dir: stat {} failed: {e}", path.display()))?;
    if !metadata.is_dir() {
        return Err(format!("list_dir: {} is not a directory", path.display()));
    }

    let mut entries: Vec<(String, &'static str, u64)> = Vec::new();
    let read = fs::read_dir(&path)
        .map_err(|e| format!("list_dir: read {} failed: {e}", path.display()))?;
    for entry in read {
        let entry = entry.map_err(|e| format!("list_dir: entry error: {e}"))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        // Use file_type() (does NOT follow links) for classification, not
        // metadata() (which follows). Windows legacy junction points like
        // "My Documents" or "Application Data" are reparse points whose
        // targets are ACL-locked; metadata() fails on them and used to
        // bucket them as `("unknown", 0)` or — worse — as `("file", 0)`
        // depending on the error path. file_type() reports them as
        // symlinks correctly.
        let (kind, size) = match entry.file_type() {
            Ok(ft) if ft.is_symlink() => ("symlink", 0),
            Ok(ft) if ft.is_dir() => ("dir", 0),
            Ok(ft) if ft.is_file() => {
                // Only stat real files for size — metadata() can be
                // expensive (or fail with permission errors) on Windows.
                let size = entry.metadata().map_or(0, |m| m.len());
                ("file", size)
            }
            Ok(_) => ("other", 0),
            Err(_) => ("unknown", 0),
        };
        entries.push((name, kind, size));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    let total = entries.len();
    let truncated = total > MAX_LIST_ENTRIES;
    if truncated {
        entries.truncate(MAX_LIST_ENTRIES);
    }

    let json_entries: Vec<Value> = entries
        .iter()
        .map(|(name, kind, size)| {
            json!({
                "name": name,
                "type": kind,
                "size": size,
            })
        })
        .collect();

    Ok(json!({
        "path": path.display().to_string(),
        "count": total,
        "truncated": truncated,
        "entries": json_entries,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `f` with `CLAUDETTE_WORKSPACE` forced to `val` (None = unset) under
    /// the global env lock, restoring the previous value afterwards. The
    /// code-file routing gate reads this var, so the refusal tests must pin it
    /// rather than inherit whatever the developer's shell happens to have set.
    fn with_workspace_env<R>(val: Option<&str>, f: impl FnOnce() -> R) -> R {
        let _guard = crate::test_env_lock();
        let prev = std::env::var("CLAUDETTE_WORKSPACE").ok();
        match val {
            Some(v) => std::env::set_var("CLAUDETTE_WORKSPACE", v),
            None => std::env::remove_var("CLAUDETTE_WORKSPACE"),
        }
        let out = f();
        match prev {
            Some(p) => std::env::set_var("CLAUDETTE_WORKSPACE", p),
            None => std::env::remove_var("CLAUDETTE_WORKSPACE"),
        }
        out
    }

    #[test]
    fn write_file_snapshots_only_existing_targets() {
        // NOTE: env mutation happens inside with_temp_home (which already
        // holds the global env lock) — do NOT nest with_workspace_env here,
        // the lock is not reentrant.
        crate::with_temp_home(|home| {
            let prev_ws = std::env::var("CLAUDETTE_WORKSPACE").ok();
            std::env::remove_var("CLAUDETTE_WORKSPACE");

            let trash = home.join(".claudette").join("trash");
            // Fresh file (lands in ~/.claudette/files under the temp home):
            // nothing to preserve → no trash entry.
            run_write_file(r#"{"path":"fresh.txt","content":"v1"}"#).unwrap();
            let trash_count = || -> usize { std::fs::read_dir(&trash).map_or(0, Iterator::count) };
            assert_eq!(trash_count(), 0, "new file must not create a pre-image");

            // Overwrite → exactly one pre-image holding the OLD content.
            run_write_file(r#"{"path":"fresh.txt","content":"v2"}"#).unwrap();
            let entries: Vec<_> = std::fs::read_dir(&trash)
                .unwrap()
                .map(|e| e.unwrap().path())
                .collect();
            assert_eq!(entries.len(), 1, "overwrite must snapshot the pre-image");
            assert_eq!(
                std::fs::read_to_string(&entries[0]).unwrap(),
                "v1",
                "pre-image must hold the truncated content"
            );

            match prev_ws {
                Some(v) => std::env::set_var("CLAUDETTE_WORKSPACE", v),
                None => std::env::remove_var("CLAUDETTE_WORKSPACE"),
            }
        });
    }

    #[test]
    fn write_file_accepts_code_directly() {
        // The generate_code coder sidecar was retired — write_file now writes
        // source files itself, no size gate and no workspace requirement.
        // Use a `.sh` file so no external interpreter is involved.
        with_workspace_env(None, || {
            let input = json!({ "path": "claudette-writecode-test.sh", "content": "echo hi\n" })
                .to_string();
            let target = files_dir().join("claudette-writecode-test.sh");
            let _ = fs::remove_file(&target);
            let out = run_write_file(&input);
            let landed = target.exists();
            let _ = fs::remove_file(&target);
            let out = out.expect("code file should be written directly");
            assert!(out.contains("\"ok\":true"), "got: {out}");
            assert!(
                landed,
                "file must land in files_dir, got nothing at {}",
                target.display()
            );
        });
    }

    #[test]
    fn write_file_accepts_large_code_directly() {
        // A large source file is no longer refused/routed anywhere — it writes.
        with_workspace_env(None, || {
            let body = "x = 1\n".repeat(200);
            let input =
                json!({ "path": "claudette-writecode-big.sh", "content": body }).to_string();
            let target = files_dir().join("claudette-writecode-big.sh");
            let _ = fs::remove_file(&target);
            let out = run_write_file(&input);
            let landed = target.exists();
            let _ = fs::remove_file(&target);
            let out = out.expect("large code file should be written directly");
            assert!(out.contains("\"ok\":true"), "got: {out}");
            assert!(
                landed,
                "file must land in files_dir, got nothing at {}",
                target.display()
            );
        });
    }

    #[test]
    fn read_file_caps_default_window_and_flags_truncation() {
        // A file bigger than the default window must come back truncated, with
        // the paging notice — this is the fix for the large-file read spiral.
        let _guard = crate::test_env_lock(); // home-resolving: serialize vs temp-home swaps
        let target = files_dir().join("claudette-readcap-test.txt");
        let _ = ensure_dir(&files_dir()); // fresh runner: files_dir may not exist yet
        let _ = fs::remove_file(&target);
        let body = (1..=1000)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&target, &body).unwrap();

        let input = json!({ "path": target.to_str().unwrap() }).to_string();
        let out = run_read_file(&input).expect("read should succeed");
        let v: Value = serde_json::from_str(&out).unwrap();

        assert_eq!(v["truncated"], json!(true), "big file must be truncated");
        assert_eq!(v["total_lines"], json!(1000));
        let content = v["content"].as_str().unwrap();
        assert!(content.contains("line 1\n"), "shows the start");
        assert!(
            !content.contains("line 500"),
            "stops before the default cap"
        );
        assert!(
            content.contains("re-read with offset="),
            "includes the paging notice: {content}"
        );

        let _ = fs::remove_file(&target);
    }

    #[test]
    fn read_file_honors_offset_and_limit() {
        let _guard = crate::test_env_lock(); // home-resolving
        let target = files_dir().join("claudette-readwin-test.txt");
        let _ = ensure_dir(&files_dir()); // fresh runner: files_dir may not exist yet
        let _ = fs::remove_file(&target);
        let body = (1..=100)
            .map(|n| format!("L{n}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&target, &body).unwrap();

        let input =
            json!({ "path": target.to_str().unwrap(), "offset": 10, "limit": 3 }).to_string();
        let out = run_read_file(&input).expect("read should succeed");
        let v: Value = serde_json::from_str(&out).unwrap();
        let content = v["content"].as_str().unwrap();

        assert_eq!(v["lines_shown"], json!("10-12"));
        assert!(content.contains("L10\nL11\nL12"), "exact window: {content}");
        assert!(!content.contains("L9"), "excludes before offset");
        assert!(!content.contains("L13"), "excludes after limit");

        let _ = fs::remove_file(&target);
    }

    #[test]
    fn read_file_tail_returns_last_lines_and_rejects_offset() {
        let _guard = crate::test_env_lock(); // home-resolving
        let target = files_dir().join("claudette-readtail-test.txt");
        let _ = ensure_dir(&files_dir()); // fresh runner: files_dir may not exist yet
        let _ = fs::remove_file(&target);
        let body = (1..=100)
            .map(|n| format!("L{n}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(&target, &body).unwrap();

        // tail=3 → the last three lines, 98-100, and nothing earlier.
        let input = json!({ "path": target.to_str().unwrap(), "tail": 3 }).to_string();
        let out = run_read_file(&input).expect("read should succeed");
        let v: Value = serde_json::from_str(&out).unwrap();
        let content = v["content"].as_str().unwrap();
        assert_eq!(v["lines_shown"], json!("98-100"), "tail window: {out}");
        assert_eq!(v["total_lines"], json!(100));
        assert!(content.contains("L98\nL99\nL100"), "exact tail: {content}");
        assert!(
            !content.contains("L97"),
            "excludes before the tail: {content}"
        );
        // end == total in tail mode, so no paging notice fires.
        assert_eq!(v["truncated"], json!(false), "tail reaches EOF: {out}");

        // tail larger than the file → whole file, no panic.
        let input_big = json!({ "path": target.to_str().unwrap(), "tail": 500 }).to_string();
        let v_big: Value = serde_json::from_str(&run_read_file(&input_big).unwrap()).unwrap();
        assert_eq!(
            v_big["lines_shown"],
            json!("1-100"),
            "tail>=len → whole file"
        );

        // tail + offset together is rejected with a clear error.
        let input_bad =
            json!({ "path": target.to_str().unwrap(), "tail": 3, "offset": 10 }).to_string();
        let err = run_read_file(&input_bad).unwrap_err();
        assert!(
            err.contains("mutually exclusive"),
            "tail+offset must be rejected: {err}"
        );

        let _ = fs::remove_file(&target);
    }

    #[test]
    fn read_file_small_file_returns_whole_without_notice() {
        let _guard = crate::test_env_lock(); // home-resolving
        let target = files_dir().join("claudette-readsmall-test.txt");
        let _ = ensure_dir(&files_dir()); // fresh runner: files_dir may not exist yet
        let _ = fs::remove_file(&target);
        fs::write(&target, "alpha\nbeta\ngamma\n").unwrap();

        let input = json!({ "path": target.to_str().unwrap() }).to_string();
        let out = run_read_file(&input).expect("read should succeed");
        let v: Value = serde_json::from_str(&out).unwrap();

        assert_eq!(v["truncated"], json!(false));
        assert_eq!(v["total_lines"], json!(3));
        assert_eq!(v["content"], json!("alpha\nbeta\ngamma\n"));

        let _ = fs::remove_file(&target);
    }

    #[test]
    fn write_file_resolves_bare_relative_under_sandbox() {
        // Regression for the dolphins-post.txt bug: the model said
        // write_file("dolphins.txt", ...) and expected it to land in the
        // sandbox. Previously the path got resolved against CWD (typically
        // the workspace root) and the sandbox check rejected it. Now bare
        // relative paths are rooted at files_dir() so the model's intuition
        // works without it having to know the sandbox path.
        with_workspace_env(None, || {
            let target = files_dir().join("claudette-relative-test.txt");
            let _ = fs::remove_file(&target);

            let input = json!({
                "path": "claudette-relative-test.txt",
                "content": "wrote via bare relative path",
            })
            .to_string();
            let out = run_write_file(&input).expect("relative write should succeed under sandbox");
            assert!(out.contains("\"ok\":true"), "got: {out}");
            assert!(target.exists(), "expected {} to exist", target.display());
            let content = fs::read_to_string(&target).unwrap();
            assert_eq!(content, "wrote via bare relative path");

            let _ = fs::remove_file(&target);
        });
    }

    #[test]
    fn write_file_still_rejects_absolute_outside_sandbox() {
        // Bare-relative resolution under the sandbox MUST NOT loosen the
        // sandbox check itself: an absolute path under the user's home but
        // outside ~/.claudette/files/ should still be rejected.
        let _guard = crate::test_env_lock(); // home-resolving
        let outside = super::super::user_home()
            .join("Documents")
            .join("definitely-not-allowed.txt");
        let input = json!({
            "path": outside.to_str().unwrap(),
            "content": "should be rejected",
        })
        .to_string();
        let result = run_write_file(&input);
        assert!(result.is_err(), "expected reject, got {result:?}");
        assert!(result.unwrap_err().contains("sandboxed"));
    }

    #[test]
    fn write_file_writes_python_without_workspace() {
        // No workspace set — code files are written directly now (the old
        // "refuse code → generate_code" routing was retired with the sidecar).
        with_workspace_env(None, || {
            let target = files_dir().join("write_code_user.py");
            let _ = fs::remove_file(&target);
            let input = json!({ "path": "write_code_user.py", "content": "x = 1\n" }).to_string();
            let out = run_write_file(&input).expect(".py should be written directly");
            assert!(out.contains("\"ok\":true"), "got: {out}");
            assert!(target.exists(), "file must land in files_dir");
            let _ = fs::remove_file(&target);
        });
    }

    #[test]
    fn write_file_writes_rust_without_workspace() {
        with_workspace_env(None, || {
            let target = files_dir().join("write_code_lib.rs");
            let _ = fs::remove_file(&target);
            let input =
                json!({ "path": "write_code_lib.rs", "content": "fn main() {}\n" }).to_string();
            let out = run_write_file(&input).expect(".rs should be written directly");
            assert!(out.contains("\"ok\":true"), "got: {out}");
            let _ = fs::remove_file(&target);
        });
    }

    #[test]
    fn write_file_allows_text_extension() {
        with_workspace_env(None, || {
            let target = files_dir().join("write_refuse_allows_txt.txt");
            let _ = fs::remove_file(&target);
            let input = json!({
                "path": "write_refuse_allows_txt.txt",
                "content": "plain notes",
            })
            .to_string();
            let out = run_write_file(&input).expect(".txt should be allowed");
            assert!(out.contains("\"ok\":true"), "got: {out}");
            assert!(
                target.exists(),
                "file must land in files_dir, got nothing at {}",
                target.display()
            );
            let _ = fs::remove_file(&target);
        });
    }

    #[test]
    fn write_file_allows_data_and_config_extensions() {
        // JSON, MD, YAML, TOML — config/data formats stay on write_file.
        with_workspace_env(None, || {
            for (path, content) in [
                ("write_refuse_data.json", r#"{"k":"v"}"#),
                ("write_refuse_data.md", "# heading"),
                ("write_refuse_data.yaml", "k: v"),
                ("write_refuse_data.toml", "k = 'v'"),
            ] {
                let target = files_dir().join(path);
                let _ = fs::remove_file(&target);
                let input = json!({ "path": path, "content": content }).to_string();
                let out = run_write_file(&input)
                    .unwrap_or_else(|e| panic!("{path} should be allowed, got: {e}"));
                assert!(out.contains("\"ok\":true"), "{path}: got {out}");
                assert!(
                    target.exists(),
                    "file must land in files_dir, got nothing at {}",
                    target.display()
                );
                let _ = fs::remove_file(&target);
            }
        });
    }

    #[test]
    fn read_file_round_trip_through_handlers() {
        // Write a file via run_write_file then read it back via run_read_file.
        // Cleans up after itself.
        let _guard = crate::test_env_lock(); // home-resolving
        let path = files_dir().join("claudette-test-roundtrip.txt");
        let _ = fs::remove_file(&path);

        let write_input = json!({
            "path": path.to_str().unwrap(),
            "content": "hello from a unit test",
        })
        .to_string();
        let write_out = run_write_file(&write_input).expect("write_file should succeed");
        assert!(write_out.contains("\"ok\":true"));

        let read_input = json!({ "path": path.to_str().unwrap() }).to_string();
        let read_out = run_read_file(&read_input).expect("read_file should succeed");
        assert!(read_out.contains("hello from a unit test"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn schemas_lists_three_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 3);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["read_file", "write_file", "list_dir"]);
    }
}
