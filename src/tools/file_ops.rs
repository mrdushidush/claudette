//! File ops group — 3 tools (read_file, write_file, list_dir).
//!
//! Sandboxing policy (set by the `validate_read_path` / `validate_write_path`
//! helpers in the parent module):
//!   - read_file / list_dir: allowed anywhere under the user's $HOME.
//!   - write_file: allowed ONLY under ~/.claudette/files/ (the scratch dir).
//!
//! write_file refuses files whose extension looks like source code —
//! those get routed to `generate_code` so the 30b coder + Codet
//! validation pipeline kicks in instead of the 4b brain writing tiny
//! stubs. Config/data formats (json, toml, yaml, md, txt, xml, ini)
//! stay on write_file because the brain can write those coherently.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::{ensure_dir, files_dir, validate_read_path, validate_write_path, MAX_FILE_BYTES};

const MAX_LIST_ENTRIES: usize = 200;

/// File extensions that `write_file` refuses, redirecting the brain to
/// `generate_code` instead (Sprint 13.3 — bulletproof code routing).
///
/// Strict subset of `REF_EXTENSIONS`: only languages where the brain has no
/// business writing the file directly. Config/data formats (json, toml, yaml,
/// md, txt, xml, ini, cfg, conf) stay on `write_file` because the brain CAN
/// write those coherently and they don't go through Codet validation.
///
/// Why refuse: the 4b brain produces tiny stubs for code, bypassing the 30b
/// coder + Codet validation entirely. Sprint 13.3 v3 task #55 collapsed to a
/// 747-byte 2-function stub of a 12-function module via this exact path.
const CODE_EXTENSIONS: &[&str] = &[
    "py", "rs", "js", "mjs", "cjs", "jsx", "ts", "tsx", "html", "htm", "css", "go", "java", "c",
    "cpp", "cc", "cxx", "h", "hpp", "rb", "php", "sh", "bash", "sql",
];

fn is_code_extension(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            let lower = e.to_ascii_lowercase();
            CODE_EXTENSIONS.contains(&lower.as_str())
        })
}

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "read_file",
                "description": "Read a text file under the user's home directory (max 100 KB).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path (absolute or ~/)" }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "write_file",
                "description": "Write plain text / config / data to ~/.claudette/files/ (notes, JSON, YAML, TOML, MD, TXT, XML, INI). REFUSES code files (.py .rs .js .ts .html .css .go .java .c .cpp .rb .php .sh .sql etc) — for code you MUST use generate_code instead so the specialised coder + validator pipeline runs.",
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

    let path = validate_read_path(path_str)?;

    let metadata = fs::metadata(&path)
        .map_err(|e| format!("read_file: stat {} failed: {e}", path.display()))?;
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

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("read_file: read {} failed: {e}", path.display()))?;

    Ok(json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": size,
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

    // Refuse code files. The brain (small, generalist) routinely writes
    // tiny code stubs that bypass the 30b coder + Codet validation. Force
    // the call through `generate_code` so the quality pipeline kicks in.
    // Brain reads the structured error and reroutes on the next turn.
    if is_code_extension(path_str) {
        return Err(format!(
            "write_file refuses code files (extension on '{path_str}'). \
             Use `generate_code` instead — it routes through the specialised \
             coder model and validates syntax+tests. Pass any existing files \
             the new code should match in `reference_files` so the coder \
             reads the real API."
        ));
    }

    if content.len() > MAX_FILE_BYTES {
        return Err(format!(
            "write_file: content is {} bytes, exceeds {MAX_FILE_BYTES}-byte limit",
            content.len()
        ));
    }

    // Bare relative paths get resolved under the sandbox dir, NOT against
    // CWD. Reasoning: the model says "save it to dolphins-post.txt" and
    // expects it to land somewhere reasonable; resolving against
    // claudette's CWD (typically the workspace root) puts it outside
    // the sandbox and the call fails. By rooting bare relative paths under
    // ~/.claudette/files/ we make the most-common case Just Work.
    // Absolute and ~/-prefixed paths still flow through validate_write_path
    // unchanged so the user can still explicitly target a sub-folder.
    let resolved_input = if Path::new(path_str).is_absolute()
        || path_str.starts_with("~/")
        || path_str.starts_with("~\\")
    {
        path_str.to_string()
    } else {
        files_dir().join(path_str).display().to_string()
    };
    let path = validate_write_path(&resolved_input)?;

    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    fs::write(&path, content)
        .map_err(|e| format!("write_file: write {} failed: {e}", path.display()))?;

    let mut result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": content.len(),
    });

    // Codet post-write hook: if the file looks like code, validate it.
    // The validation is synchronous — it may hot-swap models in Ollama
    // (Claudette ↔ coder) and take 10-30 seconds for the full
    // parse→test→fix loop. The result is folded into the tool output so
    // Claudette sees it without any extra context cost.
    if let Some(validation) = crate::codet::validate_code_file(&path, &[]) {
        result["validation"] = validation.to_json();

        // Surface a warning directly to the user (stderr) if Codet
        // couldn't fix something — the user should know even if Claudette
        // decides not to mention it.
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

    #[test]
    fn is_code_extension_classifies_correctly() {
        // Pure code → refuse.
        for ext in ["py", "rs", "js", "ts", "html", "css", "go", "sh"] {
            assert!(
                is_code_extension(&format!("file.{ext}")),
                "{ext} should be classified as code"
            );
        }
        // Config/data → allow.
        for ext in ["json", "toml", "yaml", "md", "txt", "xml", "ini"] {
            assert!(
                !is_code_extension(&format!("file.{ext}")),
                "{ext} should NOT be classified as code"
            );
        }
        // No extension → allow.
        assert!(!is_code_extension("README"));
    }

    #[test]
    fn write_file_resolves_bare_relative_under_sandbox() {
        // Regression for the dolphins-post.txt bug: the model said
        // write_file("dolphins.txt", ...) and expected it to land in the
        // sandbox. Previously the path got resolved against CWD (typically
        // the workspace root) and the sandbox check rejected it. Now bare
        // relative paths are rooted at files_dir() so the model's intuition
        // works without it having to know the sandbox path.
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
    }

    #[test]
    fn write_file_still_rejects_absolute_outside_sandbox() {
        // Bare-relative resolution under the sandbox MUST NOT loosen the
        // sandbox check itself: an absolute path under the user's home but
        // outside ~/.claudette/files/ should still be rejected.
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
    fn write_file_refuses_python_extension() {
        let input = json!({ "path": "user.py", "content": "x = 1\n" }).to_string();
        let err = run_write_file(&input).unwrap_err();
        assert!(err.contains("refuses code"), "got: {err}");
        assert!(
            err.contains("generate_code"),
            "must mention generate_code: {err}"
        );
        // File must NOT have been written.
        assert!(!files_dir().join("user.py").exists());
    }

    #[test]
    fn write_file_refuses_rust_extension() {
        let input = json!({ "path": "lib.rs", "content": "fn main() {}\n" }).to_string();
        let err = run_write_file(&input).unwrap_err();
        assert!(err.contains("refuses code"), "got: {err}");
    }

    #[test]
    fn write_file_refuses_uppercase_code_extension() {
        // Extension matching is case-insensitive.
        let input = json!({ "path": "App.HTML", "content": "<p>x</p>" }).to_string();
        let err = run_write_file(&input).unwrap_err();
        assert!(err.contains("refuses code"), "got: {err}");
    }

    #[test]
    fn write_file_allows_text_extension() {
        let target = files_dir().join("write_refuse_allows_txt.txt");
        let _ = fs::remove_file(&target);
        let input = json!({
            "path": "write_refuse_allows_txt.txt",
            "content": "plain notes",
        })
        .to_string();
        let out = run_write_file(&input).expect(".txt should be allowed");
        assert!(out.contains("\"ok\":true"), "got: {out}");
        let _ = fs::remove_file(&target);
    }

    #[test]
    fn write_file_allows_data_and_config_extensions() {
        // JSON, MD, YAML, TOML — config/data formats stay on write_file.
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
            let _ = fs::remove_file(&target);
        }
    }

    #[test]
    fn read_file_round_trip_through_handlers() {
        // Write a file via run_write_file then read it back via run_read_file.
        // Cleans up after itself.
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
