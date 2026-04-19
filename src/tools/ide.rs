//! IDE group — fire-and-forget subprocesses that hand control to the user's
//! OS shell: VS Code, the file manager, the browser.
//!
//! Three tools, no shared network state, no Codet involvement. Platform
//! branches are `#[cfg(target_os = "windows")]` vs Unix (`xdg-open`).
//!
//! Parent-module helpers used: `validate_read_path` (so we don't launch the
//! editor or the file manager on a path outside the user's home/cwd).

use std::path::Path;

use serde_json::{json, Value};

use super::validate_read_path;

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "open_in_editor",
                "description": "Open a file in the default editor (invokes `code` on PATH), optionally at a line number.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File path" },
                        "line": { "type": "number", "description": "Line number (optional)" }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "reveal_in_explorer",
                "description": "Show a file or folder in the system file manager.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "File or folder path" }
                    },
                    "required": ["path"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "open_url",
                "description": "Open a URL or local file in the default browser. Accepts http/https URLs, file:// URIs, or absolute local file paths (e.g. C:\\Users\\...\\page.html).",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": { "type": "string", "description": "URL (http/https/file://) or absolute local file path" }
                    },
                    "required": ["url"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "open_in_editor" => run_open_in_editor(input),
        "reveal_in_explorer" => run_reveal_in_explorer(input),
        "open_url" => run_open_url(input),
        _ => return None,
    };
    Some(result)
}

fn run_open_in_editor(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("open_in_editor: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("open_in_editor: missing 'path'")?;
    let line = v.get("line").and_then(Value::as_u64);

    let resolved = validate_read_path(path_str)?;
    let target = match line {
        Some(n) => format!("{}:{n}", resolved.display()),
        None => resolved.display().to_string(),
    };

    // On Windows, the default editor binary installs as `code.cmd` which
    // `Command::new("code")` can't find (CreateProcessW doesn't honour
    // PATHEXT). Use `cmd /C` wrapper on Windows to let the shell resolve it.
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "code", "--goto", &target])
            .spawn()
            .map_err(|e| format!("open_in_editor: failed to launch editor: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("code")
            .arg("--goto")
            .arg(&target)
            .spawn()
            .map_err(|e| format!("open_in_editor: failed to launch editor: {e}"))?;
    }

    Ok(json!({
        "ok": true,
        "opened": target,
    })
    .to_string())
}

fn run_reveal_in_explorer(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("reveal_in_explorer: invalid JSON ({e}): {input}"))?;
    let path_str = v
        .get("path")
        .and_then(Value::as_str)
        .ok_or("reveal_in_explorer: missing 'path'")?;

    let resolved = validate_read_path(path_str)?;

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", resolved.display()))
            .spawn()
            .map_err(|e| format!("reveal_in_explorer: failed to launch explorer: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        // macOS: open -R; Linux: xdg-open on parent dir
        let parent = resolved.parent().unwrap_or(&resolved);
        std::process::Command::new("xdg-open")
            .arg(parent.as_os_str())
            .spawn()
            .map_err(|e| format!("reveal_in_explorer: failed to open file manager: {e}"))?;
    }

    Ok(json!({
        "ok": true,
        "revealed": resolved.display().to_string(),
    })
    .to_string())
}

fn run_open_url(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("open_url: invalid JSON ({e}): {input}"))?;
    let url = v
        .get("url")
        .and_then(Value::as_str)
        .ok_or("open_url: missing 'url'")?;

    // Accept http(s), file:// URIs, and bare local paths (e.g. HTML files
    // generated by Codet). Windows `start` and Linux `xdg-open` handle all
    // three — the old http-only guard prevented opening local files.
    let is_url =
        url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://");
    if !is_url && !Path::new(url).exists() {
        return Err(format!("open_url: not a URL or existing local file: {url}"));
    }
    let target = url;

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", target])
            .spawn()
            .map_err(|e| format!("open_url: failed to open: {e}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new("xdg-open")
            .arg(&target)
            .spawn()
            .map_err(|e| format!("open_url: failed to open: {e}"))?;
    }

    Ok(json!({
        "ok": true,
        "opened": target,
    })
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_editor_rejects_missing_path() {
        let err = run_open_in_editor("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn reveal_in_explorer_rejects_missing_path() {
        let err = run_reveal_in_explorer("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn open_url_rejects_missing_url() {
        let err = run_open_url("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn open_url_rejects_bare_string_that_is_not_a_path() {
        let err = run_open_url(r#"{"url":"not-a-url-nor-a-file"}"#).unwrap_err();
        assert!(err.contains("not a URL"), "got: {err}");
    }

    #[test]
    fn schemas_lists_three_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 3);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["open_in_editor", "reveal_in_explorer", "open_url"]);
    }
}
