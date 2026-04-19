//! Notes group — 4 tools (note_create, note_list, note_read, note_delete).
//!
//! Storage: one `.md` file per note under `~/.claudette/notes/`. The
//! filename is `{ISO timestamp}-{slug}.md` so the ISO prefix gives a
//! natural newest-first sort without a separate index.
//!
//! Format: a Markdown file with a `# title` heading, optional `Created:`
//! and `Tags:` metadata lines, a blank line, then the body. Consistent
//! enough to parse back out in note_list / note_read, loose enough that
//! the user can edit the files by hand and the parser still works.
//!
//! Self-contained: `notes_dir` (pub(super) so get_capabilities can show
//! it) and `slugify` are private to this module. Handlers reuse the
//! parent-module `ensure_dir` helper.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::ensure_dir;

pub(super) fn notes_dir() -> PathBuf {
    super::claudette_home().join("notes")
}

/// Convert a title into a filesystem-safe slug. Lowercase, alphanumerics
/// and hyphens only, hyphens collapsed, max 40 chars.
fn slugify(text: &str) -> String {
    let raw: String = text
        .chars()
        .map(|c| {
            if c.is_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let collapsed: String = raw
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let trimmed: String = collapsed.chars().take(40).collect();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "note_create",
                "description": "Save a note with a title, body, and optional tags.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "description": "Note title" },
                        "body":  { "type": "string", "description": "Note content" },
                        "tags":  { "type": "string", "description": "Comma-separated tags (e.g. 'work,project,urgent')" }
                    },
                    "required": ["title", "body"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "note_list",
                "description": "List saved notes with titles, previews, and tags. Optionally filter by tag or search substring, and limit results.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "tag":    { "type": "string", "description": "Filter by tag (case-insensitive)" },
                        "search": { "type": "string", "description": "Substring match against title or body (case-insensitive)" },
                        "limit":  { "type": "integer", "description": "Maximum notes to return (default 50)" }
                    },
                    "required": []
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "note_read",
                "description": "Read the full body of a saved note by its id (filename returned from note_list).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Note id from note_list (e.g. '2026-04-14T10-30-45-meeting.md')" }
                    },
                    "required": ["id"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "note_delete",
                "description": "Delete a note by its id (filename from note_list). This is irreversible.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string", "description": "Note id from note_list" }
                    },
                    "required": ["id"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "note_create" => run_note_create(input),
        "note_list" => run_note_list(input),
        "note_read" => run_note_read(input),
        "note_delete" => run_note_delete(input),
        _ => return None,
    };
    Some(result)
}

fn run_note_create(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("note_create: invalid JSON ({e}): {input}"))?;
    let title = v
        .get("title")
        .and_then(Value::as_str)
        .ok_or("note_create: missing 'title'")?
        .to_string();
    let body = v
        .get("body")
        .and_then(Value::as_str)
        .ok_or("note_create: missing 'body'")?
        .to_string();
    let tags_str = v.get("tags").and_then(Value::as_str).unwrap_or("");
    let tags: Vec<&str> = tags_str
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    ensure_dir(&notes_dir())?;
    let now = chrono::Local::now();
    let ts = now.format("%Y-%m-%dT%H-%M-%S").to_string();
    let slug = slugify(&title);
    let filename = format!("{ts}-{slug}.md");
    let path = notes_dir().join(&filename);

    use std::fmt::Write;
    let mut content = format!("# {title}\n\nCreated: {}\n", now.to_rfc3339());
    if !tags.is_empty() {
        let _ = writeln!(content, "Tags: {}", tags.join(", "));
    }
    let _ = writeln!(content, "\n{body}");
    fs::write(&path, content).map_err(|e| format!("note_create: write failed: {e}"))?;

    let mut result = json!({
        "ok": true,
        "id": filename,
        "path": path.display().to_string(),
        "title": title,
    });
    if !tags.is_empty() {
        result["tags"] = json!(tags);
    }
    Ok(result.to_string())
}

fn run_note_list(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input).unwrap_or(json!({}));
    let filter_tag = v
        .get("tag")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let search = v
        .get("search")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let limit = v
        .get("limit")
        .and_then(Value::as_u64)
        .map_or(50, |n| n as usize);

    let dir = notes_dir();
    if !dir.exists() {
        return Ok(json!({ "count": 0, "notes": [] }).to_string());
    }

    // Collect (filename, title, tags, preview, body_for_search).
    let mut entries: Vec<(String, String, Vec<String>, String, String)> = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| format!("read notes dir: {e}"))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        let content = fs::read_to_string(&path).unwrap_or_default();
        let title = content.lines().find(|l| l.starts_with("# ")).map_or_else(
            || filename.clone(),
            |l| l.trim_start_matches("# ").to_string(),
        );
        let tags: Vec<String> = content
            .lines()
            .find(|l| l.starts_with("Tags:"))
            .map(|l| {
                l.trim_start_matches("Tags:")
                    .split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // Apply tag filter.
        if let Some(ref ft) = filter_tag {
            if !tags.iter().any(|t| t.to_lowercase() == *ft) {
                continue;
            }
        }
        // Apply search filter against title + full content (case-insensitive).
        if let Some(ref q) = search {
            let hay = format!("{}\n{}", title, content).to_lowercase();
            if !hay.contains(q) {
                continue;
            }
        }

        let preview: String = content
            .lines()
            .find(|l| {
                !(l.starts_with('#')
                    || l.starts_with("Created:")
                    || l.starts_with("Tags:")
                    || l.trim().is_empty())
            })
            .map(|s| s.chars().take(80).collect::<String>())
            .unwrap_or_default();
        entries.push((filename, title, tags, preview, content));
    }
    // Newest first by filename (ISO timestamp prefix sorts naturally)
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    let total = entries.len();
    entries.truncate(limit);

    let json_entries: Vec<Value> = entries
        .iter()
        .enumerate()
        .map(|(i, (id, title, tags, preview, _))| {
            let mut entry = json!({
                "index": i + 1,
                "id": id,
                "title": title,
                "preview": preview,
            });
            if !tags.is_empty() {
                entry["tags"] = json!(tags);
            }
            entry
        })
        .collect();

    let mut result = json!({
        "count": json_entries.len(),
        "total": total,
        "notes": json_entries,
    });
    if let Some(ref ft) = filter_tag {
        result["filtered_by_tag"] = json!(ft);
    }
    if let Some(ref q) = search {
        result["search"] = json!(q);
    }
    if total > json_entries.len() {
        result["truncated"] = json!(true);
    }
    Ok(result.to_string())
}

fn run_note_read(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("note_read: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("note_read: missing 'id' (filename from note_list)")?;
    // Reject path separators — id must be a bare filename.
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!("note_read: invalid id '{id}' (must be a filename)"));
    }
    let path = notes_dir().join(id);
    if !path.exists() {
        return Err(format!("note_read: no note with id '{id}'"));
    }
    let content = fs::read_to_string(&path).map_err(|e| format!("note_read: read failed: {e}"))?;

    let title = content.lines().find(|l| l.starts_with("# ")).map_or_else(
        || id.to_string(),
        |l| l.trim_start_matches("# ").to_string(),
    );
    let created = content
        .lines()
        .find(|l| l.starts_with("Created:"))
        .map(|l| l.trim_start_matches("Created:").trim().to_string())
        .unwrap_or_default();
    let tags: Vec<String> = content
        .lines()
        .find(|l| l.starts_with("Tags:"))
        .map(|l| {
            l.trim_start_matches("Tags:")
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    // Body = everything after the metadata block. Skip lines that start with
    // `#`, `Created:`, `Tags:`, or are blank. The first non-metadata line and
    // everything after it is the body.
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_body = false;
    for line in content.lines() {
        if !in_body {
            let is_meta = line.starts_with('#')
                || line.starts_with("Created:")
                || line.starts_with("Tags:")
                || line.trim().is_empty();
            if is_meta {
                continue;
            }
            in_body = true;
        }
        body_lines.push(line);
    }
    let body = body_lines.join("\n").trim_end().to_string();

    let mut result = json!({
        "ok": true,
        "id": id,
        "title": title,
        "body": body,
    });
    if !created.is_empty() {
        result["created"] = json!(created);
    }
    if !tags.is_empty() {
        result["tags"] = json!(tags);
    }
    Ok(result.to_string())
}

fn run_note_delete(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("note_delete: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("note_delete: missing 'id' (filename from note_list)")?;
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!(
            "note_delete: invalid id '{id}' (must be a filename)"
        ));
    }
    let path = notes_dir().join(id);
    if !path.exists() {
        return Err(format!("note_delete: no note with id '{id}'"));
    }
    fs::remove_file(&path).map_err(|e| format!("note_delete: remove failed: {e}"))?;
    Ok(json!({ "ok": true, "id": id, "deleted": true }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Call mom tomorrow"), "call-mom-tomorrow");
        assert_eq!(slugify("  --weird///title!!!  "), "weird-title");
        assert_eq!(slugify(""), "untitled");
        assert_eq!(slugify("!!!"), "untitled");
    }

    #[test]
    fn note_read_rejects_path_traversal() {
        let err = run_note_read(r#"{"id":"../secret.md"}"#).unwrap_err();
        assert!(err.contains("invalid id"), "got: {err}");
    }

    #[test]
    fn note_read_rejects_directory_separator() {
        let err = run_note_read(r#"{"id":"subdir/note.md"}"#).unwrap_err();
        assert!(err.contains("invalid id"), "got: {err}");
    }

    #[test]
    fn note_read_rejects_missing_id() {
        let err = run_note_read("{}").unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn note_read_rejects_nonexistent_note() {
        let err = run_note_read(r#"{"id":"9999-01-01T00-00-00-no-such-note.md"}"#).unwrap_err();
        assert!(err.contains("no note with id"), "got: {err}");
    }

    #[test]
    fn note_delete_rejects_path_traversal() {
        let err = run_note_delete(r#"{"id":"../boom.md"}"#).unwrap_err();
        assert!(err.contains("invalid id"), "got: {err}");
    }

    #[test]
    fn note_delete_rejects_missing_id() {
        let err = run_note_delete("{}").unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn note_delete_rejects_nonexistent() {
        let err = run_note_delete(r#"{"id":"9999-01-01T00-00-00-no-such.md"}"#).unwrap_err();
        assert!(err.contains("no note with id"), "got: {err}");
    }

    #[test]
    fn note_list_accepts_limit_and_search_without_error() {
        // Parameters should be accepted even with no notes in the filesystem.
        let out = run_note_list(r#"{"limit":5,"search":"xyz-no-match"}"#).expect("ok");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["count"].is_number());
    }

    #[test]
    fn note_list_empty_tag_is_ignored() {
        // Regression: qwen3:8b sometimes sends `{"tag": ""}` or `{"tag": "   "}`
        // for a plain "list my notes". An empty filter must not exclude every
        // note — it must behave the same as no filter at all.
        let stamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
        let title = format!("__tag_empty_test_{stamp}");
        let create_out = run_note_create(
            &json!({ "title": title, "body": "x", "tags": "anything" }).to_string(),
        )
        .expect("note_create");
        let created: Value = serde_json::from_str(&create_out).unwrap();
        let note_id = created["id"].as_str().unwrap().to_string();

        for empty in ["", "   ", "\t"] {
            let out = run_note_list(&json!({ "tag": empty }).to_string()).expect("note_list");
            let v: Value = serde_json::from_str(&out).unwrap();
            assert!(
                v["count"].as_u64().unwrap() >= 1,
                "empty tag {empty:?} should not filter everything: {v}"
            );
            assert!(
                v.get("filtered_by_tag").is_none(),
                "empty tag should not report filtered_by_tag: {v}"
            );
        }

        // Cleanup.
        let _ = run_note_delete(&json!({ "id": note_id }).to_string());
    }

    #[test]
    fn schemas_lists_four_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 4);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names,
            ["note_create", "note_list", "note_read", "note_delete"]
        );
    }
}
