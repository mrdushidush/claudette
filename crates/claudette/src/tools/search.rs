//! Search group — filesystem globbing/greping + a single-URL web_fetch.
//!
//! `glob_search` and `grep_search` are sandboxed under the user's $HOME.
//! `web_fetch` is unrestricted scheme-wise to http/https but has an 8 KB
//! output cap so a giant page can't blow the context window.
//!
//! Parent-module helpers used: validate_read_path, user_home, normalize_path,
//! strip_html, MAX_FILE_BYTES. `expand_tilde` is pub(crate) already.

use std::fs;
use std::path::Path;

use serde_json::{json, Value};

use super::{
    default_workspace_root, normalize_path, path_is_allowed, strip_html, user_home,
    validate_read_path, wrap_untrusted, WorkspaceRoots, MAX_FILE_BYTES,
};

const MAX_GLOB_RESULTS: usize = 200;
const MAX_GREP_MATCHES: usize = 100;
const MAX_GREP_FILES: usize = 5000;
const MAX_GREP_LINE_CHARS: usize = 200;

/// Directories grep_search never descends into (build output, dep caches, VCS
/// metadata). Shared with repo_map via the parent module so the two code-search
/// tools stay in lockstep. Without it, a single grep on a Rust/Node project
/// drowns in `target/` or `node_modules/` and hits the file cap before reaching
/// the source tree — the observed cause of the q3 "locate" spiral.
use super::SEARCH_SKIP_DIRS as SKIP_DIRS;
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
                "description": "Search file contents with a regular expression (ripgrep-style, case-insensitive) under a directory. Defaults to the active workspace/project; skips build & dependency dirs (target, node_modules, .git). Use a precise pattern like CLAUDETTE_MAX_FIX_ROUNDS or fn\\s+max_rounds.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regex to search for (e.g. 'TODO|FIXME', 'fn\\s+\\w+'). Invalid regex falls back to literal substring." },
                        "path":    { "type": "string", "description": "Directory to search (default: the workspace/project root)" }
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
        // Bare-relative pattern. Resolve under the same root priority
        // grep_search uses, so `glob_search("**/foo.py")` searches the
        // project the user pointed claudette at — not their whole $HOME:
        //   1. active mission tree,
        //   2. the CLAUDETTE_WORKSPACE root (cwd's root if cwd is inside one,
        //      else the first root) — this is the daily-driver coding path,
        //   3. $HOME (ad-hoc "find me PDFs" searches with no workspace set).
        // Pre-fix this fell straight through to $HOME, so a workspace on
        // another drive (e.g. D:\repo while $HOME is C:\Users\...) was never
        // searched and the brain read decoy files from $HOME instead.
        let base = if crate::missions::active_mission().is_some() {
            crate::missions::active_cwd()
        } else if let Some(root) = default_workspace_root() {
            root
        } else {
            user_home()
        };
        base.join(raw_pattern).display().to_string()
    };

    // Sandbox check on the literal prefix (everything before the first glob
    // metachar). The literal prefix is the part of the path glob will
    // actually walk into; if THAT escapes the allowed envelope we reject.
    // Without this check the user could pass `../etc/**/*` and walk out.
    // The envelope is the same one grep_search / validate_read_path enforce:
    // $HOME + cwd-if-under-home + CLAUDETTE_WORKSPACE roots — so a workspace
    // on another drive is allowed, but arbitrary filesystem walks are not.
    let prefix_end = resolved_pattern
        .find(['*', '?', '['])
        .unwrap_or(resolved_pattern.len());
    let literal_prefix = &resolved_pattern[..prefix_end];
    let literal_path = normalize_path(Path::new(literal_prefix));
    let roots = WorkspaceRoots::from_env();
    if !path_is_allowed(&literal_path, &roots, false) {
        return Err(format!(
            "glob_search: pattern resolves outside the allowed roots — $HOME ({}) \
             and CLAUDETTE_WORKSPACE; searches are restricted for safety",
            roots.home.display()
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
    // Default search root, in priority order:
    // 1. Active mission tree (matches the brain's likely intent — grep
    //    within the project being worked on).
    // 2. Process cwd if it's inside a CLAUDETTE_WORKSPACE root (so a user
    //    who launched claudette from inside their workspace gets the
    //    project, not their home dir).
    // 3. First CLAUDETTE_WORKSPACE root (so a user who launched from $HOME
    //    with CLAUDETTE_WORKSPACE=D:/dev/foo gets the project they
    //    pointed claudette at).
    // 4. $HOME (pre-T2 default).
    //
    // The pre-fix default of `~` is F5: it caused grep_search to silently
    // crawl HOME when the brain meant "grep the project I'm working on".
    let default_path: String;
    let path_str = match v.get("path").and_then(Value::as_str) {
        Some(s) => s,
        None => {
            default_path = if let Some(m) = crate::missions::active_mission() {
                m.path.display().to_string()
            } else if let Some(root) = crate::tools::default_workspace_root() {
                root.display().to_string()
            } else {
                "~".to_string()
            };
            default_path.as_str()
        }
    };

    let root = validate_read_path(path_str)?;
    let metadata = fs::metadata(&root)
        .map_err(|e| format!("grep_search: stat {} failed: {e}", root.display()))?;
    if !metadata.is_dir() {
        return Err(format!(
            "grep_search: {} is not a directory",
            root.display()
        ));
    }

    // Compile the pattern as a case-insensitive regex (ripgrep semantics —
    // what coding models reach for: alternation, `.?`, char classes). If it
    // isn't valid regex (a small brain occasionally passes a raw string with
    // stray metacharacters), fall back to a literal case-insensitive
    // substring match so the search still does something useful.
    let regex = regex::RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .ok();
    let mode = if regex.is_some() { "regex" } else { "literal" };
    let needle = pattern.to_lowercase();
    let line_matches = |line: &str| -> bool {
        match &regex {
            Some(re) => re.is_match(line),
            None => line.to_lowercase().contains(&needle),
        }
    };
    let mut matches: Vec<Value> = Vec::new();
    let mut files_scanned: usize = 0;
    let mut truncated = false;

    // Walk with ripgrep's `ignore` crate: respects .gitignore / .ignore,
    // skips hidden files, and never descends into VCS metadata. `filter_entry`
    // additionally prunes build/dependency dirs even when the project has no
    // .gitignore (SKIP_DIRS), so a plain folder of code still doesn't crawl
    // target/ or node_modules/. This is what stops the search from drowning in
    // *.log build logs and target/ artifacts (the observed q3 spiral cause).
    let walker = ignore::WalkBuilder::new(&root)
        .hidden(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .follow_links(false)
        .filter_entry(|entry| {
            if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                let name = entry.file_name().to_string_lossy();
                if SKIP_DIRS.contains(&name.as_ref()) {
                    return false;
                }
            }
            true
        })
        .build();

    'walk: for result in walker {
        let Ok(entry) = result else { continue };
        // Only regular files (the walker also yields directories + the root).
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        // Bail-out checked per-file so we always finish the current file's
        // matches before stopping.
        if files_scanned >= MAX_GREP_FILES {
            truncated = true;
            break 'walk;
        }
        files_scanned += 1;
        let p = entry.path();

        // Skip oversized files — same 100 KB cap as read_file.
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES as u64 {
            continue;
        }
        // Read as text; binary files fail UTF-8 and get skipped.
        let Ok(content) = fs::read_to_string(p) else {
            continue;
        };
        for (lineno, line) in content.lines().enumerate() {
            if line_matches(line) {
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

    Ok(json!({
        "pattern": pattern,
        "mode": mode,
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
    fn glob_search_allows_workspace_outside_home() {
        // Regression: glob_search used to root at $HOME and reject any pattern
        // resolving elsewhere, so a workspace on another drive (D:\repo while
        // $HOME is C:\Users\...) was unreachable — the brain then read decoy
        // files from $HOME. The sandbox now honours CLAUDETTE_WORKSPACE, the
        // same envelope grep_search uses. Invented non-existent root so the
        // glob walker simply yields nothing (Ok) — we assert the *gate*, not
        // file matching, which keeps this independent of FS contents.
        #[cfg(unix)]
        let (root, pat) = ("/claudette-glob-ws-xyz", "/claudette-glob-ws-xyz/**/*.py");
        #[cfg(not(unix))]
        let (root, pat) = (
            r"Z:\claudette-glob-ws-xyz",
            r"Z:\claudette-glob-ws-xyz\**\*.py",
        );
        let input = json!({ "pattern": pat }).to_string();

        let _guard = crate::test_env_lock();
        let prev = std::env::var("CLAUDETTE_WORKSPACE").ok();

        // No workspace set → pattern is outside $HOME → rejected.
        std::env::remove_var("CLAUDETTE_WORKSPACE");
        let denied = run_glob_search(&input);

        // Workspace points at the root → the sandbox accepts it.
        std::env::set_var("CLAUDETTE_WORKSPACE", root);
        let allowed = run_glob_search(&input);

        // Restore env before asserting so a panic can't poison other tests.
        match prev {
            Some(v) => std::env::set_var("CLAUDETTE_WORKSPACE", v),
            None => std::env::remove_var("CLAUDETTE_WORKSPACE"),
        }

        assert!(
            denied.is_err(),
            "outside $HOME with no workspace must be rejected: {denied:?}"
        );
        assert!(
            allowed.is_ok(),
            "a CLAUDETTE_WORKSPACE root outside $HOME must be allowed: {allowed:?}"
        );
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
    fn grep_search_uses_regex_and_skips_build_dirs() {
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-greptest-x7q");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("src")).unwrap();
        fs::create_dir_all(base.join("target")).unwrap();
        fs::write(
            base.join("src").join("run.rs"),
            "fn max_fix_rounds() -> u32 { 3 }\nconst CLAUDETTE_MAX_FIX_ROUNDS: u32 = 3;\n",
        )
        .unwrap();
        // Same symbol in a build artifact — must be skipped, not returned.
        fs::write(
            base.join("target").join("junk.rs"),
            "CLAUDETTE_MAX_FIX_ROUNDS in a build artifact\n",
        )
        .unwrap();

        // Regex alternation + `.?` (the exact shape the q3 brain wrote) matches.
        let input = json!({
            "pattern": "max.?fix.?rounds|MAX_FIX_ROUNDS",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_grep_search(&input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["mode"], json!("regex"), "got: {out}");
        assert!(
            v["match_count"].as_u64().unwrap() >= 1,
            "regex should match the source symbol: {out}"
        );
        let files: Vec<String> = v["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["file"].as_str().unwrap().replace('\\', "/"))
            .collect();
        assert!(
            files.iter().any(|f| f.contains("/src/run.rs")),
            "should find src/run.rs: {files:?}"
        );
        assert!(
            !files.iter().any(|f| f.contains("/target/")),
            "must skip target/: {files:?}"
        );

        // Unbalanced paren → invalid regex → literal-substring fallback.
        let input2 =
            json!({ "pattern": "max_fix_rounds(", "path": base.to_str().unwrap() }).to_string();
        let out2 = run_grep_search(&input2).unwrap();
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(v2["mode"], json!("literal"), "got: {out2}");

        let _ = fs::remove_dir_all(&base);
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
