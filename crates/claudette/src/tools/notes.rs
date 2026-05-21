//! Notes group — 4 tools (note_create, note_list, note_read, note_delete).
//! v0.6.0 dropped the standalone note_update — note_create is now an
//! upsert: pass `id` to update an existing note, omit it to create a new
//! one. The old `note_update` name keeps dispatching through an alias
//! shim for one release.
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
                "description": "Create or update a note (upsert). Omit `id` to create new (title + body required). Pass `id` from note_list to update an existing note — only the fields you provide change.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "id":    { "type": "string", "description": "Optional: existing note id (filename from note_list). When provided, this is an update." },
                        "title": { "type": "string", "description": "Note title (required on create, optional on update)" },
                        "body":  { "type": "string", "description": "Note content (required on create, optional on update)" },
                        "tags":  { "type": "string", "description": "Comma-separated tags (e.g. 'work,urgent'). Empty string clears on update." }
                    },
                    "required": []
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
        // v0.6.0 deprecated alias — drop in next minor release. Old shape
        // is `{id, title?, body?, tags?}` and matches the inner update
        // path exactly, so we just forward.
        "note_update" => run_note_update(input),
        "note_delete" => run_note_delete(input),
        _ => return None,
    };
    Some(result)
}

fn run_note_create(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("note_create: invalid JSON ({e}): {input}"))?;

    // Upsert semantics (v0.6.0): if `id` is present, route to the update
    // path. This subsumes the dropped note_update tool — note_create now
    // covers "create new" and "update existing" through one entry point.
    if v.get("id").and_then(Value::as_str).is_some() {
        return run_note_update(input);
    }

    let title = v
        .get("title")
        .and_then(Value::as_str)
        .ok_or("note_create: missing 'title' (required when creating; pass 'id' to update an existing note)")?
        .to_string();
    let body = v
        .get("body")
        .and_then(Value::as_str)
        .ok_or("note_create: missing 'body' (required when creating; pass 'id' to update an existing note)")?
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
                    || l.starts_with("Updated:")
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
    // `#`, `Created:`, `Updated:`, `Tags:`, or are blank. The first non-
    // metadata line and everything after it is the body. `Updated:` was
    // added by note_update — older notes without it round-trip unchanged.
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_body = false;
    for line in content.lines() {
        if !in_body {
            let is_meta = line.starts_with('#')
                || line.starts_with("Created:")
                || line.starts_with("Updated:")
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

fn run_note_update(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("note_update: invalid JSON ({e}): {input}"))?;
    let id = v
        .get("id")
        .and_then(Value::as_str)
        .ok_or("note_update: missing 'id' (filename from note_list)")?;
    if id.contains('/') || id.contains('\\') || id.contains("..") {
        return Err(format!(
            "note_update: invalid id '{id}' (must be a filename)"
        ));
    }

    // `Some(s)` — caller wants to set this field. `None` — leave alone.
    // Tags is the only field where the empty string carries meaning ("clear
    // all tags"), so it's tracked separately from the title/body presence
    // checks. JSON `null` and a missing key both collapse to `None` here.
    let new_title = v.get("title").and_then(Value::as_str).map(String::from);
    let new_body = v.get("body").and_then(Value::as_str).map(String::from);
    let new_tags = v.get("tags").and_then(Value::as_str).map(String::from);

    if new_title.is_none() && new_body.is_none() && new_tags.is_none() {
        return Err(
            "note_update: nothing to update (pass at least one of title, body, tags)".to_string(),
        );
    }

    let path = notes_dir().join(id);
    if !path.exists() {
        return Err(format!("note_update: no note with id '{id}'"));
    }

    let original = fs::read_to_string(&path)
        .map_err(|e| format!("note_update: read {} failed: {e}", path.display()))?;

    // Parse the existing note into its three structural pieces — heading,
    // metadata block (Created:/Tags:/Updated:), and body — so we can rewrite
    // only the parts the caller targeted. We deliberately preserve `Created:`
    // and refresh `Updated:` rather than rewriting the whole header from
    // scratch — keeps user-edited files round-trippable.
    let existing_title = original.lines().find(|l| l.starts_with("# ")).map_or_else(
        || id.to_string(),
        |l| l.trim_start_matches("# ").to_string(),
    );
    let existing_created = original
        .lines()
        .find(|l| l.starts_with("Created:"))
        .map(|l| l.trim_start_matches("Created:").trim().to_string())
        .unwrap_or_default();
    let existing_tags: Vec<String> = original
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
    let mut existing_body_lines: Vec<&str> = Vec::new();
    let mut in_body = false;
    for line in original.lines() {
        if !in_body {
            let is_meta = line.starts_with('#')
                || line.starts_with("Created:")
                || line.starts_with("Updated:")
                || line.starts_with("Tags:")
                || line.trim().is_empty();
            if is_meta {
                continue;
            }
            in_body = true;
        }
        existing_body_lines.push(line);
    }
    let existing_body = existing_body_lines.join("\n").trim_end().to_string();

    // Resolve the final values — caller-supplied wins, else carry-over.
    let final_title = new_title.unwrap_or(existing_title);
    let final_body = new_body.unwrap_or(existing_body);
    let final_tags: Vec<String> = match new_tags {
        Some(s) => s
            .split(',')
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect(),
        None => existing_tags,
    };

    // Rebuild the file content. Updated: gets the current timestamp on every
    // update. Created: stays unchanged if present, otherwise omitted (the
    // caller may have hand-rolled the file without one).
    let now = chrono::Local::now();
    use std::fmt::Write;
    let mut content = format!("# {final_title}\n\n");
    if !existing_created.is_empty() {
        let _ = writeln!(content, "Created: {existing_created}");
    }
    let _ = writeln!(content, "Updated: {}", now.to_rfc3339());
    if !final_tags.is_empty() {
        let _ = writeln!(content, "Tags: {}", final_tags.join(", "));
    }
    let _ = writeln!(content, "\n{final_body}");

    // Atomic write — write a sibling tmp file, fsync, rename. A crash mid-
    // write leaves the original intact; the .tmp gets cleaned up next time
    // the user (or this code) writes the same note. Same shape as edit_file.
    let tmp = path.with_extension("claudette-update.tmp");
    fs::write(&tmp, &content)
        .map_err(|e| format!("note_update: write tmp {} failed: {e}", tmp.display()))?;
    fs::rename(&tmp, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("note_update: rename to {} failed: {e}", path.display())
    })?;

    let mut result = json!({
        "ok": true,
        "id": id,
        "title": final_title,
        "updated": now.to_rfc3339(),
    });
    if !final_tags.is_empty() {
        result["tags"] = json!(final_tags);
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

    #[test]
    fn note_create_without_id_still_requires_title_and_body() {
        let err = run_note_create("{}").unwrap_err();
        assert!(err.contains("missing 'title'"), "got: {err}");
        let err = run_note_create(r#"{"title":"x"}"#).unwrap_err();
        assert!(err.contains("missing 'body'"), "got: {err}");
    }

    #[test]
    fn note_create_with_id_is_upsert() {
        // Create a fresh note, then update it by passing `id` back into
        // note_create. The on-disk content must reflect the new body and
        // the filename (id) must be unchanged.
        let id = create_note_for_update_test("upsert_test", "first body", "");
        let upsert_out = run_note_create(
            &json!({ "id": &id, "body": "second body via note_create" }).to_string(),
        )
        .expect("note_create upsert");
        let v: Value = serde_json::from_str(&upsert_out).unwrap();
        assert_eq!(v["id"].as_str().unwrap(), id);

        let read = run_note_read(&json!({ "id": &id }).to_string()).expect("note_read");
        let r: Value = serde_json::from_str(&read).unwrap();
        assert_eq!(r["body"].as_str().unwrap(), "second body via note_create");

        let _ = run_note_delete(&json!({ "id": &id }).to_string());
    }

    #[test]
    fn note_update_alias_still_dispatches() {
        // The old note_update tool must keep working through the dispatch
        // alias. Exercise via the public dispatcher to prove the alias
        // arm is wired even though the schema no longer advertises it.
        let id = create_note_for_update_test("alias_test", "before", "");
        let out = super::dispatch(
            "note_update",
            &json!({ "id": &id, "body": "after via alias" }).to_string(),
        )
        .expect("note_update alias must dispatch")
        .expect("note_update alias must succeed");
        assert!(out.contains("\"ok\":true"));

        let read = run_note_read(&json!({ "id": &id }).to_string()).expect("note_read");
        let r: Value = serde_json::from_str(&read).unwrap();
        assert_eq!(r["body"].as_str().unwrap(), "after via alias");

        let _ = run_note_delete(&json!({ "id": &id }).to_string());
    }

    // ── note_update ─────────────────────────────────────────────────────

    /// Helper: create a note and return its id. Caller is responsible for
    /// deletion at the end of the test (try a `let _ = run_note_delete(...)`
    /// in a defer-style cleanup).
    fn create_note_for_update_test(title: &str, body: &str, tags: &str) -> String {
        let stamp = chrono::Local::now().timestamp_nanos_opt().unwrap_or(0);
        let unique = format!("{title}_{stamp}");
        let out =
            run_note_create(&json!({ "title": unique, "body": body, "tags": tags }).to_string())
                .expect("note_create");
        let v: Value = serde_json::from_str(&out).unwrap();
        v["id"].as_str().unwrap().to_string()
    }

    #[test]
    fn note_update_rejects_path_traversal() {
        let err = run_note_update(r#"{"id":"../boom.md","body":"x"}"#).unwrap_err();
        assert!(err.contains("invalid id"), "got: {err}");
    }

    #[test]
    fn note_update_rejects_missing_id() {
        let err = run_note_update(r#"{"body":"x"}"#).unwrap_err();
        assert!(err.contains("missing 'id'"), "got: {err}");
    }

    #[test]
    fn note_update_rejects_nonexistent_note() {
        let err =
            run_note_update(r#"{"id":"9999-01-01T00-00-00-no-such.md","body":"x"}"#).unwrap_err();
        assert!(err.contains("no note with id"), "got: {err}");
    }

    #[test]
    fn note_update_rejects_no_fields_to_update() {
        // Existing note still required so this gate fires *before* the
        // "nothing to update" gate would matter. Using a guaranteed-bogus id
        // to make the test order-independent.
        let id = create_note_for_update_test("nothing_to_update", "body", "");
        let err = run_note_update(&json!({ "id": id }).to_string()).unwrap_err();
        assert!(
            err.contains("nothing to update"),
            "expected nothing-to-update error, got: {err}"
        );
        let _ = run_note_delete(&json!({ "id": id }).to_string());
    }

    #[test]
    fn note_update_replaces_body_only() {
        let id = create_note_for_update_test("body_only", "first body", "tag-a");
        let out = run_note_update(&json!({ "id": id, "body": "second body" }).to_string())
            .expect("note_update");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["ok"], true);

        // Read back and confirm body replaced, tags unchanged, title unchanged.
        let read = run_note_read(&json!({ "id": id }).to_string()).expect("note_read");
        let r: Value = serde_json::from_str(&read).unwrap();
        assert_eq!(r["body"].as_str().unwrap(), "second body");
        assert!(r["tags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t.as_str() == Some("tag-a")));

        let _ = run_note_delete(&json!({ "id": id }).to_string());
    }

    #[test]
    fn note_update_title_change_keeps_filename_updates_heading() {
        let id = create_note_for_update_test("title_change", "body", "");
        let out = run_note_update(&json!({ "id": id, "title": "Renamed Title" }).to_string())
            .expect("note_update");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["title"].as_str().unwrap(), "Renamed Title");

        // The id (filename) must be unchanged — that's the brain's stable
        // handle. The `# heading` line inside the file is what got rewritten.
        let read = run_note_read(&json!({ "id": id }).to_string()).expect("note_read");
        let r: Value = serde_json::from_str(&read).unwrap();
        assert_eq!(r["id"].as_str().unwrap(), id);
        assert_eq!(r["title"].as_str().unwrap(), "Renamed Title");

        let _ = run_note_delete(&json!({ "id": id }).to_string());
    }

    #[test]
    fn note_update_tags_replace_not_merge() {
        let id = create_note_for_update_test("tags_replace", "body", "old-a,old-b");
        let _ = run_note_update(&json!({ "id": id, "tags": "new-only" }).to_string())
            .expect("note_update");
        let read = run_note_read(&json!({ "id": id }).to_string()).expect("note_read");
        let r: Value = serde_json::from_str(&read).unwrap();
        let tags: Vec<&str> = r["tags"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t.as_str())
            .collect();
        assert_eq!(tags, vec!["new-only"], "tags must replace, not merge");

        let _ = run_note_delete(&json!({ "id": id }).to_string());
    }

    #[test]
    fn note_update_empty_tags_string_clears_tags() {
        let id = create_note_for_update_test("tags_clear", "body", "to-clear");
        let _ = run_note_update(&json!({ "id": id, "tags": "" }).to_string()).expect("note_update");
        let read = run_note_read(&json!({ "id": id }).to_string()).expect("note_read");
        let r: Value = serde_json::from_str(&read).unwrap();
        // Empty tags should cause the `tags` field to be absent in the
        // response (matching note_read's existing convention) — the on-disk
        // `Tags:` line is also dropped.
        assert!(
            r.get("tags").is_none(),
            "empty tags string should clear all tags, got: {r}"
        );

        let _ = run_note_delete(&json!({ "id": id }).to_string());
    }

    #[test]
    fn note_update_preserves_created_adds_updated_line() {
        let id = create_note_for_update_test("updated_line", "body", "");
        let path = notes_dir().join(&id);
        let before = fs::read_to_string(&path).expect("read original");
        let created_line = before
            .lines()
            .find(|l| l.starts_with("Created:"))
            .expect("note_create writes a Created: line")
            .to_string();

        let _ = run_note_update(&json!({ "id": id, "body": "after" }).to_string())
            .expect("note_update");
        let after = fs::read_to_string(&path).expect("read updated");

        assert!(
            after.contains(&created_line),
            "Created: line must be preserved verbatim across updates"
        );
        assert!(
            after.lines().any(|l| l.starts_with("Updated:")),
            "Updated: line must be added on update: {after}"
        );

        let _ = run_note_delete(&json!({ "id": id }).to_string());
    }
}
