//! Shell + edit group — 2 tools (bash, edit_file).
//!
//! These are the DangerFullAccess tools: bash can run arbitrary shell
//! commands; edit_file can modify files under the user's $HOME (broader
//! than write_file's ~/.claudette/files/ sandbox). Both require explicit
//! user confirmation at dispatch time — Sprint 2c will wire
//! PermissionMode::Prompt; for now the confirmation is the up-front
//! choice the user made when enabling these tools in config.
//!
//! Self-contained: `BASH_OUTPUT_MAX_CHARS` is private. Handlers reuse
//! the parent-module `validate_read_path` (pub(super)) for edit_file's
//! path gate, and `run_command_with_timeout` from crate::test_runner
//! directly for bash's subprocess.

use std::fs;

use serde_json::{json, Value};

use super::validate_read_path;
use crate::test_runner::run_command_with_timeout;

const BASH_OUTPUT_MAX_CHARS: usize = 8192;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "bash",
                "description": "Run a shell command. Requires user confirmation. Use for system tasks the other tools can't handle.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to execute" }
                    },
                    "required": ["command"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "edit_file",
                "description": "Replace text in an existing file under the user's home. Requires confirmation. For creating new files use write_file or generate_code.",
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
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "bash" => run_bash(input),
        "edit_file" => run_edit_file(input),
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
    #[cfg(target_os = "windows")]
    let (program, args) = ("cmd", vec!["/C", command]);
    #[cfg(not(target_os = "windows"))]
    let (program, args) = ("sh", vec!["-c", command]);

    let result = run_command_with_timeout(program, &args, 30, None);

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

    // $HOME-gated (broader than write_file's sandbox) because the user
    // explicitly confirmed via the permission prompt.
    let path = validate_read_path(path_str)?;

    let content = fs::read_to_string(&path)
        .map_err(|e| format!("edit_file: read {} failed: {e}", path.display()))?;

    // Count occurrences: 0 → clear error, 1 → replace, >1 → refuse instead
    // of silently taking the first match. An ambiguous edit against a
    // large file is the easy way to corrupt it quietly.
    let match_count = content.matches(old_text).count();
    match match_count {
        0 => {
            return Err(format!(
                "edit_file: old_text not found in {}. The text to replace must match exactly.",
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
    fn schemas_lists_two_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 2);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["bash", "edit_file"]);
    }
}
