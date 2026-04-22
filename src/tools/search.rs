//! Search group — filesystem globbing/greping + a single-URL web_fetch.
//!
//! `glob_search` and `grep_search` are sandboxed under the user's $HOME.
//! `web_fetch` is unrestricted scheme-wise to http/https but has an 8 KB
//! output cap so a giant page can't blow the context window.
//!
//! Parent-module helpers used: validate_read_path, user_home, normalize_path,
//! strip_html, MAX_FILE_BYTES. `expand_tilde` is pub(crate) already.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{
    normalize_path, strip_html, user_home, validate_read_path, wrap_untrusted, MAX_FILE_BYTES,
};

const MAX_GLOB_RESULTS: usize = 200;
const MAX_GREP_MATCHES: usize = 50;
const MAX_GREP_FILES: usize = 200;
const MAX_GREP_LINE_CHARS: usize = 200;
const WEB_FETCH_MAX_CHARS: usize = 8192;
const WEB_FETCH_TIMEOUT_SECS: u64 = 15;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "web_fetch",
                "description": "Fetch a URL and return cleaned visible text (HTML stripped, max 8 KB).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL to fetch (http/https)" }
                    },
                    "required": ["url"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "glob_search",
                "description": "Find files by glob pattern under the user's home (e.g. **/*.py).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Glob pattern (e.g. '**/*.py', 'Downloads/*.pdf')" }
                    },
                    "required": ["pattern"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "grep_search",
                "description": "Search file contents for a substring (case-insensitive) under a directory.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Text to search for" },
                        "path":    { "type": "string", "description": "Directory to search (default: home)" }
                    },
                    "required": ["pattern"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "glob_search" => run_glob_search(input),
        "grep_search" => run_grep_search(input),
        "web_fetch" => run_web_fetch(input),
        _ => return None,
    };
    Some(result)
}

fn run_glob_search(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("glob_search: invalid JSON ({e}): {input}"))?;
    let raw_pattern = v
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or("glob_search: missing 'pattern'")?;

    // Resolve pattern to an absolute filesystem path. Three cases:
    //   - Absolute path → use as-is, then validate it stays under $HOME.
    //   - Tilde-prefixed → expand_tilde.
    //   - Bare relative pattern → join under $HOME.
    let resolved_pattern = if raw_pattern.starts_with("~/") || raw_pattern.starts_with("~\\") {
        crate::tools::expand_tilde(raw_pattern)
            .display()
            .to_string()
    } else if Path::new(raw_pattern).is_absolute() {
        raw_pattern.to_string()
    } else {
        user_home().join(raw_pattern).display().to_string()
    };

    // Sandbox check on the literal prefix (everything before the first glob
    // metachar). The literal prefix is the part of the path glob will
    // actually walk into; if THAT escapes $HOME we reject. Without this
    // check the user could pass `../etc/**/*` and walk outside $HOME.
    let prefix_end = resolved_pattern
        .find(['*', '?', '['])
        .unwrap_or(resolved_pattern.len());
    let literal_prefix = &resolved_pattern[..prefix_end];
    let literal_path = normalize_path(Path::new(literal_prefix));
    let home = normalize_path(&user_home());
    if !literal_path.starts_with(&home) {
        return Err(format!(
            "glob_search: pattern resolves outside $HOME ({}); searches are restricted for safety",
            home.display()
        ));
    }

    // Glob errors (bad pattern syntax) → user-facing error.
    let walker =
        glob::glob(&resolved_pattern).map_err(|e| format!("glob_search: bad pattern: {e}"))?;

    let mut paths: Vec<String> = Vec::new();
    let mut truncated = false;
    for entry in walker {
        if paths.len() >= MAX_GLOB_RESULTS {
            truncated = true;
            break;
        }
        // Permission errors and unreachable paths come back as Err — skip
        // them silently rather than failing the whole search.
        if let Ok(path) = entry {
            paths.push(path.display().to_string());
        }
    }
    paths.sort();

    Ok(json!({
        "pattern": resolved_pattern,
        "count": paths.len(),
        "truncated": truncated,
        "paths": paths,
    })
    .to_string())
}

fn run_grep_search(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("grep_search: invalid JSON ({e}): {input}"))?;
    let pattern = v
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or("grep_search: missing 'pattern'")?;
    if pattern.is_empty() {
        return Err("grep_search: pattern is empty".to_string());
    }
    let path_str = v.get("path").and_then(Value::as_str).unwrap_or("~");

    let root = validate_read_path(path_str)?;
    let metadata = fs::metadata(&root)
        .map_err(|e| format!("grep_search: stat {} failed: {e}", root.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "grep_search: {} is not a directory",
            root.display()
        ));
    }

    let needle = pattern.to_lowercase();
    let mut matches: Vec<Value> = Vec::new();
    let mut files_scanned: usize = 0;
    let mut truncated = false;

    // Iterative DFS over the directory tree. Skips hidden directories
    // (`.cache`, `.git`, etc.) so a personal-secretary grep doesn't drown
    // in dotfile noise. The MAX_GREP_FILES + MAX_GREP_MATCHES caps are the
    // belt-and-braces against runaway walks.
    let mut stack: Vec<PathBuf> = vec![root.clone()];
    'walk: while let Some(dir) = stack.pop() {
        let Ok(read) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in read {
            let Ok(entry) = entry else { continue };
            let p = entry.path();
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            // Skip hidden entries (Unix dot-prefix; we don't try to detect
            // Windows hidden attribute, that needs a separate API call).
            if name_str.starts_with('.') {
                continue;
            }
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                // Don't follow symlinks — could loop or escape sandbox.
                continue;
            }
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            // Bail-out conditions checked per-file so we always finish the
            // current entry's matches before stopping.
            if files_scanned >= MAX_GREP_FILES {
                truncated = true;
                break 'walk;
            }
            files_scanned += 1;

            // Skip oversized files — same 100 KB cap as read_file.
            let Ok(meta) = entry.metadata() else { continue };
            if meta.len() > MAX_FILE_BYTES as u64 {
                continue;
            }
            // Read as text; binary files fail UTF-8 and get skipped.
            let Ok(content) = fs::read_to_string(&p) else {
                continue;
            };
            for (lineno, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&needle) {
                    let snippet: String = line.chars().take(MAX_GREP_LINE_CHARS).collect();
                    matches.push(json!({
                        "file": p.display().to_string(),
                        "line": lineno + 1,
                        "text": snippet,
                    }));
                    if matches.len() >= MAX_GREP_MATCHES {
                        truncated = true;
                        break 'walk;
                    }
                }
            }
        }
    }

    Ok(json!({
        "pattern": pattern,
        "root": root.display().to_string(),
        "files_scanned": files_scanned,
        "match_count": matches.len(),
        "truncated": truncated,
        "matches": matches,
    })
    .to_string())
}

// ────── web_fetch ────────────────────────────────────────────────────────
//
// MVP: no scheme allowlist beyond http/https, no SSRF guard (the threat
// model is a local secretary on the user's own machine), no JS rendering.
// 8 KB cap on output keeps the context window safe even on giant pages.

fn run_web_fetch(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("web_fetch: invalid JSON ({e}): {input}"))?;
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .ok_or("web_fetch: missing 'url'")?;
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err(format!(
            "web_fetch: only http:// and https:// URLs are allowed, got: {url}"
        ));
    }

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("web_fetch: build http client: {e}"))?;

    let resp = client
        .get(url)
        .header("User-Agent", "claudette/1.0 (Claudette personal secretary)")
        .header("Accept", "text/html,application/xhtml+xml,text/plain")
        .send()
        .map_err(|e| format!("web_fetch: request failed: {e}"))?;

    let status = resp.status();
    let final_url = resp.url().to_string();
    if !status.is_success() {
        return Err(format!("web_fetch: HTTP {status} for {final_url}"));
    }
    let body = resp
        .text()
        .map_err(|e| format!("web_fetch: read body: {e}"))?;

    let cleaned = strip_html(&body);
    let total_chars = cleaned.chars().count();
    let truncated = total_chars > WEB_FETCH_MAX_CHARS;
    let visible: String = cleaned.chars().take(WEB_FETCH_MAX_CHARS).collect();

    // Wrap the page text in <untrusted source="web_fetch:URL">…</untrusted>
    // and defang any in-body attempt to close the tag. Paired with the
    // system-prompt invariant; prevents a hostile page from smuggling
    // instructions into the model's trusted context. Same defense shape as
    // Gmail's <email> wrapper.
    let wrapped = wrap_untrusted(&format!("web_fetch:{final_url}"), &visible);

    Ok(json!({
        "url": final_url,
        "status": status.as_u16(),
        "chars": visible.chars().count(),
        "total_chars": total_chars,
        "truncated": truncated,
        "text": wrapped,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_search_rejects_missing_pattern() {
        let err = run_glob_search("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn grep_search_rejects_missing_pattern() {
        let err = run_grep_search("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn grep_search_rejects_empty_pattern_inline() {
        let err = run_grep_search(r#"{"pattern":""}"#).unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn web_fetch_rejects_missing_url() {
        let err = run_web_fetch("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn web_fetch_rejects_non_http_scheme_inline() {
        let err = run_web_fetch(r#"{"url":"file:///etc/passwd"}"#).unwrap_err();
        assert!(err.contains("http://"), "got: {err}");
    }

    #[test]
    fn schemas_lists_three_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 3);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["web_fetch", "glob_search", "grep_search"]);
    }
}
