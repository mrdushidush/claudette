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

/// `grep_search`: max context lines per side (ripgrep -C). Keeps a window
/// bounded even if a model passes a silly value.
const MAX_GREP_CONTEXT: usize = 10;

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
                "description": "Search file contents with a regular expression (ripgrep-style, case-insensitive by default) under a directory. Defaults to the active workspace/project; skips build & dependency dirs (target, node_modules, .git). Use a precise pattern like CLAUDETTE_MAX_FIX_ROUNDS or fn\\s+max_rounds.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Regex to search for (e.g. 'TODO|FIXME', 'fn\\s+\\w+'). Invalid regex falls back to literal substring." },
                        "path":    { "type": "string", "description": "Directory to search (default: the workspace/project root)" },
                        "glob":    { "type": "string", "description": "Optional filename glob to restrict the search (e.g. '*.rs', 'src/**/*.ts'). A bare name matches files at any depth; a pattern with '/' matches the path relative to the search root. When omitted, all files are searched." },
                        "count_only": { "type": "boolean", "description": "When true, return only the total match count and a per-file breakdown — no line bodies, and not capped at 100. Use to gauge how widespread a pattern is. Default false." },
                        "case_sensitive": { "type": "boolean", "description": "When true, match the pattern with exact case. Default false (case-insensitive)." },
                        "context": { "type": "integer", "description": "Lines of surrounding context to include on EACH side of every match (ripgrep -C), capped at 10. Each returned line carries is_match (true = the matched line, false = context); overlapping windows are merged so no line repeats. Ignored when count_only is set. Default 0 (no context)." }
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

    // Belt (issue #25 §A): reject any `..` path component outright. The
    // literal-prefix check below only guards the part BEFORE the first glob
    // metachar, so a `..` AFTER a `*` (`<workspace>/*/../../etc/**`) slips past
    // it and `glob::glob` walks out. No legitimate search pattern contains a
    // `..` component, so refusing them closes the traversal vector cleanly.
    if resolved_pattern.split(['/', '\\']).any(|c| c == "..") {
        return Err("glob_search: '..' path components are not allowed in patterns".to_string());
    }

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
            // Re-validate every expanded path (issue #25 §A): the literal-prefix
            // check only guards the part before the first metachar, and glob
            // follows symlinks into directories, so a matched path can resolve
            // outside the envelope even with a clean prefix. glob yields real
            // on-disk paths, so canonicalise and re-check; drop escapes silently
            // like permission errors.
            let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| normalize_path(&path));
            if !path_is_allowed(&canonical, &roots, true) {
                continue;
            }
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

#[allow(clippy::too_many_lines)]
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

    // Optional glob filter — restricts which files are searched (ripgrep -g
    // style). Backslashes are normalized to `/` so a Windows-style pattern
    // like `src\*.rs` still works; the glob crate treats `\` as a literal
    // character, never an escape, so nothing is lost.
    let glob_pattern: Option<glob::Pattern> = v
        .get("glob")
        .and_then(Value::as_str)
        .map(|g| {
            glob::Pattern::new(&g.replace('\\', "/"))
                .map_err(|e| format!("grep_search: invalid glob pattern '{g}': {e}"))
        })
        .transpose()?;

    // Count-only mode: tally matches and a per-file breakdown, skip the line
    // bodies, and (below) do NOT stop at the 100-match cap so the total is the
    // true count across the same filtered set normal mode would return.
    let count_only = v
        .get("count_only")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Optional exact-case match. Default false → today's case-insensitive
    // behavior on both the regex and the literal-fallback paths.
    let case_sensitive = v
        .get("case_sensitive")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    // Optional symmetric context window (ripgrep -C). Capped; 0 = today's
    // behavior (no context, output byte-identical).
    let context = v
        .get("context")
        .and_then(Value::as_u64)
        .map_or(0, |n| (n as usize).min(MAX_GREP_CONTEXT));

    // Compile the pattern as a case-insensitive regex (ripgrep semantics —
    // what coding models reach for: alternation, `.?`, char classes). If it
    // isn't valid regex (a small brain occasionally passes a raw string with
    // stray metacharacters), fall back to a literal case-insensitive
    // substring match so the search still does something useful.
    let regex = regex::RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .size_limit(1 << 20)
        .build()
        .ok();
    let mode = if regex.is_some() { "regex" } else { "literal" };
    let needle = if case_sensitive {
        pattern.to_string()
    } else {
        pattern.to_lowercase()
    };
    let line_matches = |line: &str| -> bool {
        match &regex {
            Some(re) => re.is_match(line),
            None if case_sensitive => line.contains(&needle),
            None => line.to_lowercase().contains(&needle),
        }
    };
    let mut matches: Vec<Value> = Vec::new();
    let mut match_total: usize = 0; // matched lines only (context lines excluded)
    let mut files_scanned: usize = 0;
    let mut truncated = false;
    let mut skipped_oversize: usize = 0;
    let mut total_matches: usize = 0; // count_only: true total (no 100-match cap)
    let mut file_counts: Vec<Value> = Vec::new(); // count_only: per-file breakdown

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
        let p = entry.path();

        // Apply the optional glob filter (ripgrep `-g` semantics) before the
        // file counts as scanned, so filtered-out files don't consume the
        // MAX_GREP_FILES cap. A pattern without `/` matches the file name at
        // any depth (`*.rs` finds nested sources); a pattern with `/` matches
        // the root-relative path with literal separators, so `src/*.rs` does
        // NOT match `src/sub/mod.rs`. The latter needs
        // `require_literal_separator` — the glob crate's DEFAULT MatchOptions
        // lets `*` cross `/` on every platform.
        if let Some(ref pat) = glob_pattern {
            let matched = if pat.as_str().contains('/') {
                let rel_path = p.strip_prefix(&root).unwrap_or(p);
                // Join components with `/` — on Windows strip_prefix yields
                // backslash-separated paths, which would never match.
                let norm: String = rel_path
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                let opts = glob::MatchOptions {
                    require_literal_separator: true,
                    ..Default::default()
                };
                pat.matches_with(&norm, opts)
            } else {
                pat.matches(&entry.file_name().to_string_lossy())
            };
            if !matched {
                continue;
            }
        }
        files_scanned += 1;

        // Skip oversized files — same cap as read_file. Counted, not silently
        // dropped, so the result can flag that a too-big file went unsearched
        // instead of looking like a clean "no match" (a silent skip once made a
        // model conclude present code had been deleted).
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES as u64 {
            skipped_oversize += 1;
            continue;
        }
        // Read as text; binary files fail UTF-8 and get skipped.
        let Ok(content) = fs::read_to_string(p) else {
            continue;
        };
        let mut file_match_count: usize = 0;
        // Context mode needs random access to the file's lines; only pay for it
        // when context > 0 so the default path is unchanged.
        let ctx_lines: Vec<&str> = if context > 0 {
            content.lines().collect()
        } else {
            Vec::new()
        };
        let mut last_ctx_end: Option<usize> = None; // highest line index already emitted (dedup)
        for (lineno, line) in content.lines().enumerate() {
            if line_matches(line) {
                if count_only {
                    file_match_count += 1;
                    total_matches += 1;
                } else if context == 0 {
                    let snippet: String = line.chars().take(MAX_GREP_LINE_CHARS).collect();
                    matches.push(json!({
                        "file": p.display().to_string(),
                        "line": lineno + 1,
                        "text": snippet,
                    }));
                    match_total += 1;
                    if match_total >= MAX_GREP_MATCHES {
                        truncated = true;
                        break 'walk;
                    }
                } else {
                    // Emit [lineno-context ..= lineno+context], skipping lines an
                    // earlier overlapping window already emitted (matches run in
                    // increasing line order, so `end` is monotonic — no dup, no
                    // backward move). Each line flagged is_match.
                    let start = lineno.saturating_sub(context);
                    let end = (lineno + context).min(ctx_lines.len().saturating_sub(1));
                    let from = match last_ctx_end {
                        Some(e) if e >= start => e + 1,
                        _ => start,
                    };
                    // `from..=end` as an iterator (clippy needless_range_loop is
                    // on + CI is -D warnings, so do NOT write `for j in from..=end`
                    // with `ctx_lines[j]`). `take(end + 1).skip(from)` yields
                    // exactly indices from..=end, and is empty when from > end
                    // (the whole window was already emitted by an earlier match).
                    for (j, ctx_line) in ctx_lines.iter().enumerate().take(end + 1).skip(from) {
                        let snippet: String = ctx_line.chars().take(MAX_GREP_LINE_CHARS).collect();
                        matches.push(json!({
                            "file": p.display().to_string(),
                            "line": j + 1,
                            "text": snippet,
                            "is_match": line_matches(ctx_line),
                        }));
                    }
                    last_ctx_end = Some(end);
                    match_total += 1; // count the MATCH, not its context lines
                    if match_total >= MAX_GREP_MATCHES {
                        truncated = true;
                        break 'walk;
                    }
                }
            }
        }
        if count_only && file_match_count > 0 {
            file_counts.push(json!({
                "file": p.display().to_string(),
                "count": file_match_count,
            }));
        }
    }

    let mut result = if count_only {
        json!({
            "pattern": pattern,
            "mode": mode,
            "root": root.display().to_string(),
            "files_scanned": files_scanned,
            "count_only": true,
            "match_count": total_matches,
            "file_counts": file_counts,
            "truncated": truncated,
        })
    } else {
        json!({
            "pattern": pattern,
            "mode": mode,
            "root": root.display().to_string(),
            "files_scanned": files_scanned,
            "match_count": match_total,
            "truncated": truncated,
            "matches": matches,
        })
    };
    if skipped_oversize > 0 {
        result["skipped_oversize"] = json!(skipped_oversize);
        result["note"] = json!(format!(
            "{skipped_oversize} file(s) exceeded the {MAX_FILE_BYTES}-byte size cap and were NOT \
             searched — a low match_count may be incomplete. Open such a file directly with \
             read_file (it pages) instead of assuming the pattern is absent."
        ));
    }
    Ok(result.to_string())
}

// ────── web_fetch ────────────────────────────────────────────────────────
//
// http/https only, 8 KB output cap, no JS rendering. SSRF guard
// ([`validate_fetch_target`]) blocks loopback / private / link-local targets
// (incl. the cloud metadata endpoint) so a prompt-injected model can't pivot
// `web_fetch` into the user's LAN or a metadata service.

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
    // SSRF guard: resolve the host ONCE and validate every address it maps to.
    // The validated addresses are pinned into the client (below) so reqwest
    // connects to exactly what we checked — a DNS-rebinding answer can't swap an
    // internal IP in between the check and the TCP connect.
    let target = validate_fetch_target(url)?;
    // Offline mode: the SSRF guard above already rejects loopback/private
    // targets, so any URL that reaches here is public — block it.
    crate::egress::guard(url)?;

    let client = web_fetch_client(&target)?;

    let resp = client
        .get(url)
        .header("User-Agent", "claudette/1.0 (Claudette coding agent)")
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

/// Build the blocking HTTP client `web_fetch` uses. Two layers of SSRF defense:
///
///  * **Connection pinning** — when `target` carries validated addresses (the
///    normal case), the client resolves `target.host` to exactly those IPs via
///    `resolve_to_addrs`. reqwest then connects to the addresses we already
///    checked instead of re-resolving the name at connect time, so a low-TTL
///    DNS-rebinding answer can't slip an internal IP in between the
///    [`validate_fetch_target`] check and the socket connect (the TOCTOU window
///    the previous code left open). The `Host` header and TLS SNI still use the
///    original hostname.
///  * **Per-redirect re-validation** — a custom redirect policy re-runs
///    [`validate_fetch_target`] on every hop, so a public page returning
///    `301 → http://169.254.169.254/` (cloud metadata) or `→ 192.168.0.1` is
///    refused, and the chain is capped at 10 hops so a loop can't spin forever.
///
/// Redirect hops to a *different* host are re-resolved by reqwest for their own
/// connection: they are re-validated (a static internal redirect is blocked) but
/// not IP-pinned, so a rebinding answer on a redirect hop is a narrower residual
/// — closing it fully would mean following redirects manually. (roast 2026-06-30)
fn web_fetch_client(target: &FetchTarget) -> Result<reqwest::blocking::Client, String> {
    let policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error(std::io::Error::other("web_fetch: too many redirects"));
        }
        match validate_fetch_target(attempt.url().as_str()) {
            Ok(_) => attempt.follow(),
            // The SSRF guard rejected this redirect target — turn it into a
            // hard error so the chain stops here instead of being followed.
            Err(msg) => attempt.error(std::io::Error::other(msg)),
        }
    });
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(WEB_FETCH_TIMEOUT_SECS))
        .redirect(policy);
    if !target.addrs.is_empty() {
        // Pin the connection to the validated IP(s). Empty addrs only happens
        // under CLAUDETTE_WEB_FETCH_ALLOW_PRIVATE=1, where the user opted into
        // reaching arbitrary LAN hosts and we deliberately don't pin.
        builder = builder.resolve_to_addrs(&target.host, &target.addrs);
    }
    builder
        .build()
        .map_err(|e| format!("web_fetch: build http client: {e}"))
}

/// A validated `web_fetch` destination: the original hostname plus the resolved
/// addresses that passed the SSRF check. The addresses are pinned into the
/// client so the connection can't be rebound to an internal IP after the check.
/// `addrs` is empty only under `CLAUDETTE_WEB_FETCH_ALLOW_PRIVATE=1`, which
/// means "allowed, don't pin".
struct FetchTarget {
    host: String,
    addrs: Vec<std::net::SocketAddr>,
}

/// SSRF guard for `web_fetch`. Refuses URLs whose host is loopback, private
/// (RFC1918), carrier-grade-NAT, link-local (169.254/16 — incl. the cloud
/// metadata endpoint 169.254.169.254), or otherwise internal. Hostnames are
/// resolved so a public name pointing at an internal address is also caught.
/// Opt out with `CLAUDETTE_WEB_FETCH_ALLOW_PRIVATE=1` (users fetching their
/// own LAN services). (roast 2026-06-02 H2)
///
/// Resolution is **fail-closed** (roast 2026-06-30): a host that fails to
/// resolve, or resolves to zero addresses, is refused rather than falling
/// through to `Ok` — otherwise a name that didn't resolve at check time but
/// resolved at connect time skipped the guard. On success the validated
/// addresses are returned so the caller can pin the connection to them.
fn validate_fetch_target(url: &str) -> Result<FetchTarget, String> {
    if std::env::var("CLAUDETTE_WEB_FETCH_ALLOW_PRIVATE").as_deref() == Ok("1") {
        // Opted into LAN fetches: skip both the block-list and IP pinning (the
        // point is to reach arbitrary user-controlled hosts).
        return Ok(FetchTarget {
            host: String::new(),
            addrs: Vec::new(),
        });
    }
    // Caller already validated the scheme as lowercase http:// or https://.
    let (rest, default_port) = if let Some(r) = url.strip_prefix("https://") {
        (r, 443u16)
    } else if let Some(r) = url.strip_prefix("http://") {
        (r, 80u16)
    } else {
        // Unreachable: the caller validated the scheme. Fail closed anyway.
        return Err("web_fetch: only http:// and https:// URLs are allowed".to_string());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let (host, port) = if let Some(after) = hostport.strip_prefix('[') {
        // IPv6 literal: [::1]:port
        let Some((h, tail)) = after.split_once(']') else {
            return Err("web_fetch: malformed IPv6 host".to_string());
        };
        let port = tail
            .strip_prefix(':')
            .and_then(|p| p.parse().ok())
            .unwrap_or(default_port);
        (h.to_string(), port)
    } else if let Some((h, p)) = hostport.rsplit_once(':') {
        p.parse::<u16>().map_or_else(
            |_| (hostport.to_string(), default_port),
            |pn| (h.to_string(), pn),
        )
    } else {
        (hostport.to_string(), default_port)
    };

    if host.is_empty() {
        return Err("web_fetch: URL has no host".to_string());
    }
    let host_l = host.to_ascii_lowercase();
    if host_l == "localhost"
        || host_l.ends_with(".localhost")
        || host_l == "metadata.google.internal"
    {
        return Err(blocked_target_msg(&host));
    }

    // IP literal → check directly, pin to that single address.
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        if is_blocked_fetch_ip(&ip) {
            return Err(blocked_target_msg(&host));
        }
        return Ok(FetchTarget {
            host,
            addrs: vec![std::net::SocketAddr::new(ip, port)],
        });
    }

    // Hostname → resolve ONCE and fail closed. The old code checked addresses
    // only inside `if let Ok(addrs)` and fell through to `Ok(())` on a
    // resolution error or empty result, so an unresolvable-at-check name that
    // resolved at connect time bypassed the guard. Now that is a refusal, and
    // the validated addresses are returned to pin the connection.
    use std::net::ToSocketAddrs;
    let resolved: Vec<std::net::SocketAddr> = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("web_fetch: could not resolve host '{host}': {e}"))?
        .collect();
    if resolved.is_empty() {
        return Err(format!("web_fetch: host '{host}' resolved to no addresses"));
    }
    for addr in &resolved {
        if is_blocked_fetch_ip(&addr.ip()) {
            return Err(blocked_target_msg(&host));
        }
    }
    Ok(FetchTarget {
        host,
        addrs: resolved,
    })
}

fn blocked_target_msg(host: &str) -> String {
    format!(
        "web_fetch: refusing to fetch internal/loopback/private host '{host}' (SSRF guard; \
         set CLAUDETTE_WEB_FETCH_ALLOW_PRIVATE=1 to allow LAN fetches)"
    )
}

/// True for addresses a fetch must never reach: loopback, RFC1918 private,
/// CGNAT, link-local (incl. 169.254.169.254 metadata), unspecified, broadcast,
/// IPv6 ULA/link-local, and IPv4-mapped forms of all the above.
fn is_blocked_fetch_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || o[0] == 0
                || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64.0.0/10 CGNAT
        }
        std::net::IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|m| is_blocked_fetch_ip(&std::net::IpAddr::V4(m)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_fetch_blocks_ssrf_targets() {
        // roast 2026-06-02 H2: localhost / private / metadata are refused.
        for url in [
            "http://localhost:8080/",
            "http://127.0.0.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/",
            "http://192.168.1.1/admin",
            "http://[::1]:9000/",
            "https://metadata.google.internal/computeMetadata/v1/",
        ] {
            assert!(
                validate_fetch_target(url).is_err(),
                "expected SSRF block for {url}"
            );
        }
        // A normal public IP literal is allowed.
        assert!(validate_fetch_target("https://1.1.1.1/").is_ok());
    }

    #[test]
    fn web_fetch_client_refuses_redirect_to_internal_host() {
        // Proves the redirect policy is actually wired into the web_fetch
        // client (not just that validate_fetch_target works in isolation). A
        // loopback server answers 301 → http://169.254.169.254/ (cloud
        // metadata). The client must refuse to follow it: send() returns Err.
        //
        // We drive the client directly with a loopback *initial* URL — the
        // policy only runs on the redirect hop, so the initial loopback
        // address is reached, and the 169.254 redirect target is what the
        // policy rejects. (run_web_fetch additionally blocks loopback initial
        // URLs up front; that path is covered by the SSRF test above.)
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let server = std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf); // drain the request line
                let resp = "HTTP/1.1 301 Moved Permanently\r\n\
                            Location: http://169.254.169.254/\r\n\
                            Content-Length: 0\r\n\r\n";
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        // Build an unpinned client (empty addrs) so it can reach the loopback
        // test server; the redirect policy is what we're exercising here.
        let client = web_fetch_client(&FetchTarget {
            host: String::new(),
            addrs: Vec::new(),
        })
        .expect("build client");
        let result = client.get(format!("http://{addr}/")).send();
        let _ = server.join();

        assert!(
            result.is_err(),
            "redirect to the 169.254.169.254 metadata host must be refused"
        );
    }

    #[test]
    fn web_fetch_fails_closed_on_unresolvable_host() {
        // roast 2026-06-30: a host that doesn't resolve must be REFUSED, not
        // fall through to Ok (which let reqwest re-resolve at connect time, the
        // TOCTOU bypass). `.invalid` is reserved by RFC 6761 to never resolve.
        let r = validate_fetch_target("http://nonexistent-host.invalid/");
        assert!(r.is_err(), "an unresolvable host must fail closed, got Ok");
    }

    #[test]
    fn web_fetch_returns_pinnable_addrs_for_public_ip() {
        // A public IP literal is allowed AND carries the validated address so
        // the client can pin the connection to it (no re-resolution).
        let target = validate_fetch_target("https://1.1.1.1/").expect("public IP allowed");
        assert!(
            !target.addrs.is_empty(),
            "must return the validated address to pin"
        );
        assert!(
            target.addrs.iter().all(|a| !is_blocked_fetch_ip(&a.ip())),
            "pinned addresses must all be public"
        );
        assert!(target.addrs.iter().any(|a| a.port() == 443), "port carried");
    }

    #[test]
    fn glob_search_rejects_missing_pattern() {
        let err = run_glob_search("{}").unwrap_err();
        assert!(err.contains("missing"), "got: {err}");
    }

    #[test]
    fn glob_search_rejects_dotdot_traversal() {
        // issue #25 §A: a `..` AFTER the first glob metachar escapes the
        // literal-prefix sandbox check. The `..`-component belt rejects it
        // before glob ever walks. Absolute pattern so the test doesn't depend
        // on the workspace base.
        #[cfg(unix)]
        let pat = "/tmp/*/../../etc/*";
        #[cfg(not(unix))]
        let pat = r"C:\Users\*\..\..\Windows\*";
        let input = serde_json::json!({ "pattern": pat }).to_string();
        let err = run_glob_search(&input).unwrap_err();
        assert!(err.contains(".."), "expected '..' rejection, got: {err}");
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
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-home swaps
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
    fn grep_search_searches_large_files_and_flags_oversized() {
        let _eg = crate::test_env_lock();
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-greptest-bigfiles");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        // ~200 KB: over the OLD 100 KB cap, under the new 1 MB cap → must be
        // searched now (this is exactly the api.rs-went-invisible regression).
        let mut medium = "x".repeat(200 * 1024);
        medium.push_str("\nFINDME_NEEDLE\n");
        fs::write(base.join("medium.rs"), &medium).unwrap();

        // ~1.1 MB: over the new cap → skipped, but FLAGGED, not silent.
        let mut huge = "y".repeat(1_100 * 1024);
        huge.push_str("\nFINDME_NEEDLE\n");
        fs::write(base.join("huge.rs"), &huge).unwrap();

        let input =
            json!({ "pattern": "FINDME_NEEDLE", "path": base.to_str().unwrap() }).to_string();
        let out = run_grep_search(&input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        let files: Vec<String> = v["matches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["file"].as_str().unwrap().replace('\\', "/"))
            .collect();
        // The 200 KB file is now searched (would have been skipped at 100 KB).
        assert!(
            files.iter().any(|f| f.contains("/medium.rs")),
            "200 KB file must be searched now: {out}"
        );
        // The 1.1 MB file is skipped…
        assert!(
            !files.iter().any(|f| f.contains("/huge.rs")),
            "1.1 MB file must be skipped: {out}"
        );
        // …but flagged, not silent.
        assert_eq!(
            v["skipped_oversize"].as_u64().unwrap(),
            1,
            "the oversized file must be counted: {out}"
        );
        assert!(
            v["note"].as_str().unwrap().contains("read_file"),
            "the note must point to read_file: {out}"
        );

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

    #[test]
    fn grep_search_glob_filter_restricts_files() {
        // The `glob` parameter restricts which files are searched. Only files
        // whose workspace-relative path matches the glob pattern should be
        // scanned; others must be silently skipped.
        let _eg = crate::test_env_lock();
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-grep-glob-test");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("src")).unwrap();
        fs::write(
            base.join("src").join("main.rs"),
            "fn main() { println!(\"hello\"); }\n",
        )
        .unwrap();
        fs::write(base.join("src").join("lib.rs"), "pub fn helper() {}\n").unwrap();
        fs::write(base.join("README.md"), "# Project\n").unwrap();

        // Without glob → both source files + README are scanned (3 matches).
        let input = json!({
            "pattern": "\\w+",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_grep_search(&input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["match_count"].as_u64().unwrap(),
            3,
            "without glob should find all files"
        );

        // With glob "*.rs" → only .rs files are scanned (2 matches).
        let input_glob = json!({
            "pattern": "\\w+",
            "path": base.to_str().unwrap(),
            "glob": "*.rs"
        })
        .to_string();
        let out_glob = run_grep_search(&input_glob).unwrap();
        let v2: Value = serde_json::from_str(&out_glob).unwrap();
        assert_eq!(
            v2["match_count"].as_u64().unwrap(),
            2,
            "glob *.rs should only match .rs files"
        );

        // With glob "*.md" → only README is scanned (1 match).
        let input_md = json!({
            "pattern": "#",
            "path": base.to_str().unwrap(),
            "glob": "*.md"
        })
        .to_string();
        let out_md = run_grep_search(&input_md).unwrap();
        let v3: Value = serde_json::from_str(&out_md).unwrap();
        assert_eq!(
            v3["match_count"].as_u64().unwrap(),
            1,
            "glob *.md should only match .md files"
        );

        // With glob "**/lib.rs" → only lib.rs matches.
        let input_lib = json!({
            "pattern": "helper",
            "path": base.to_str().unwrap(),
            "glob": "**/lib.rs"
        })
        .to_string();
        let out_lib = run_grep_search(&input_lib).unwrap();
        let v4: Value = serde_json::from_str(&out_lib).unwrap();
        assert_eq!(
            v4["match_count"].as_u64().unwrap(),
            1,
            "glob **/lib.rs should only match lib.rs"
        );

        // With glob that matches nothing → zero results.
        let input_none = json!({
            "pattern": "\\w+",
            "path": base.to_str().unwrap(),
            "glob": "*.xyz"
        })
        .to_string();
        let out_none = run_grep_search(&input_none).unwrap();
        let v5: Value = serde_json::from_str(&out_none).unwrap();
        assert_eq!(
            v5["match_count"].as_u64().unwrap(),
            0,
            "glob *.xyz should match nothing"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn grep_search_case_sensitive_honors_exact_case() {
        let _eg = crate::test_env_lock();
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-grep-case-test");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        // Foo/foo exercise the regex path; Bar(/bar( exercise the literal
        // fallback ("Bar(" is invalid regex — unclosed group — so the search
        // falls back to a substring match, which must also honor the flag).
        fs::write(base.join("s.txt"), "Foo\nfoo\nBar(x)\nbar(x)\n").unwrap();

        // Default (omitted) → case-insensitive: "Foo" matches Foo AND foo.
        let out = run_grep_search(
            &json!({ "pattern": "Foo", "path": base.to_str().unwrap() }).to_string(),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["match_count"].as_u64().unwrap(),
            2,
            "default is case-insensitive"
        );

        // case_sensitive=true → "Foo" matches only the exact-case line.
        let out_cs = run_grep_search(
            &json!({
                "pattern": "Foo",
                "path": base.to_str().unwrap(),
                "case_sensitive": true
            })
            .to_string(),
        )
        .unwrap();
        let v_cs: Value = serde_json::from_str(&out_cs).unwrap();
        assert_eq!(
            v_cs["match_count"].as_u64().unwrap(),
            1,
            "case_sensitive matches exact case only"
        );

        // Literal fallback (invalid regex "Bar(") must honor case_sensitive too.
        let out_lit = run_grep_search(
            &json!({
                "pattern": "Bar(",
                "path": base.to_str().unwrap(),
                "case_sensitive": true
            })
            .to_string(),
        )
        .unwrap();
        let v_lit: Value = serde_json::from_str(&out_lit).unwrap();
        assert_eq!(
            v_lit["match_count"].as_u64().unwrap(),
            1,
            "literal fallback honors case_sensitive"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn grep_search_glob_rejects_invalid_pattern() {
        // An invalid glob pattern must produce a user-facing error.
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-grep-glob-invalid");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("test.txt"), "hello\n").unwrap();

        // Unmatched `[` is an invalid glob pattern.
        let input = json!({
            "pattern": "hello",
            "path": base.to_str().unwrap(),
            "glob": "[invalid"
        })
        .to_string();
        let err = run_grep_search(&input).unwrap_err();
        assert!(
            err.contains("invalid glob"),
            "expected invalid glob error, got: {err}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn grep_search_glob_matches_relative_path() {
        // The glob filter must match against the workspace-relative path, not
        // just the filename. A pattern like `src/*.rs` should NOT match
        // `src/sub/mod.rs`, but `**/mod.rs` should.
        let _eg = crate::test_env_lock();
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-grep-glob-rel");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("src").join("sub")).unwrap();
        fs::write(base.join("src").join("main.rs"), "fn main() {}\n").unwrap();
        fs::write(
            base.join("src").join("sub").join("mod.rs"),
            "pub fn sub() {}\n",
        )
        .unwrap();

        // `src/*.rs` matches only src/main.rs (not the nested one).
        let input = json!({
            "pattern": "\\w+",
            "path": base.to_str().unwrap(),
            "glob": "src/*.rs"
        })
        .to_string();
        let out = run_grep_search(&input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v["match_count"].as_u64().unwrap(),
            1,
            "src/*.rs should only match src/main.rs"
        );

        // `**/mod.rs` matches the nested file.
        let input2 = json!({
            "pattern": "sub",
            "path": base.to_str().unwrap(),
            "glob": "**/mod.rs"
        })
        .to_string();
        let out2 = run_grep_search(&input2).unwrap();
        let v2: Value = serde_json::from_str(&out2).unwrap();
        assert_eq!(
            v2["match_count"].as_u64().unwrap(),
            1,
            "**/mod.rs should match src/sub/mod.rs"
        );

        // `src/**/*.rs` matches both files under src/.
        let input3 = json!({
            "pattern": "\\w+",
            "path": base.to_str().unwrap(),
            "glob": "src/**/*.rs"
        })
        .to_string();
        let out3 = run_grep_search(&input3).unwrap();
        let v3: Value = serde_json::from_str(&out3).unwrap();
        assert_eq!(
            v3["match_count"].as_u64().unwrap(),
            2,
            "src/**/*.rs should match both files"
        );

        // Windows-style separators in the pattern are normalized:
        // `src\*.rs` behaves like `src/*.rs`.
        let input4 = json!({
            "pattern": "\\w+",
            "path": base.to_str().unwrap(),
            "glob": "src\\*.rs"
        })
        .to_string();
        let out4 = run_grep_search(&input4).unwrap();
        let v4: Value = serde_json::from_str(&out4).unwrap();
        assert_eq!(
            v4["match_count"].as_u64().unwrap(),
            1,
            "src\\*.rs should normalize to src/*.rs"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn grep_search_count_only_returns_totals_without_line_bodies() {
        let _eg = crate::test_env_lock();
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-grep-count-only-test");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("src")).unwrap();
        // a.rs: 150 matching lines — deliberately OVER the 100-match cap, so the
        // test proves count_only reports the TRUE total (not capped) and does
        // not flag truncation, whereas default mode caps at 100 and truncates.
        let a: String = (0..150).map(|_| "let NEEDLE = 1;\n").collect();
        fs::write(base.join("src").join("a.rs"), a).unwrap();
        // b.rs: 3 matching lines + 1 non-match.
        fs::write(
            base.join("src").join("b.rs"),
            "NEEDLE\nNEEDLE\nNEEDLE\nnot here\n",
        )
        .unwrap();

        // count_only=true → true total 153, per-file breakdown, NO line bodies.
        let input = json!({
            "pattern": "NEEDLE",
            "path": base.to_str().unwrap(),
            "count_only": true
        })
        .to_string();
        let out = run_grep_search(&input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["count_only"], json!(true), "got: {out}");
        assert_eq!(
            v["match_count"].as_u64().unwrap(),
            153,
            "count_only must report the true total across the filtered set: {out}"
        );
        assert_eq!(
            v["truncated"],
            json!(false),
            "count_only is not capped at 100 matches: {out}"
        );
        // No line bodies at all — the whole point of count_only.
        assert!(
            v.get("matches").is_none(),
            "count_only must omit the `matches` array: {out}"
        );
        assert!(
            !out.contains("\"text\""),
            "count_only must omit line bodies: {out}"
        );
        // Per-file breakdown present and correct.
        let count_for = |suffix: &str| {
            v["file_counts"].as_array().unwrap().iter().find_map(|e| {
                let f = e["file"].as_str().unwrap().replace('\\', "/");
                if f.ends_with(suffix) {
                    Some(e["count"].as_u64().unwrap())
                } else {
                    None
                }
            })
        };
        assert_eq!(count_for("src/a.rs"), Some(150), "a.rs count: {out}");
        assert_eq!(count_for("src/b.rs"), Some(3), "b.rs count: {out}");

        // Omitting the flag is unchanged: line bodies present, capped at 100 +
        // truncated, and no count_only / file_counts keys.
        let input_default = json!({
            "pattern": "NEEDLE",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out_d = run_grep_search(&input_default).unwrap();
        let vd: Value = serde_json::from_str(&out_d).unwrap();
        assert!(
            vd.get("count_only").is_none(),
            "default mode omits count_only: {out_d}"
        );
        assert!(
            vd.get("file_counts").is_none(),
            "default mode omits file_counts: {out_d}"
        );
        assert_eq!(
            vd["match_count"].as_u64().unwrap(),
            100,
            "default mode caps matches at 100: {out_d}"
        );
        assert_eq!(
            vd["truncated"],
            json!(true),
            "default mode truncates at the cap: {out_d}"
        );
        assert!(
            vd["matches"].as_array().unwrap()[0]["text"].is_string(),
            "default mode returns line bodies: {out_d}"
        );

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn grep_search_context_includes_flagged_surrounding_lines() {
        let _eg = crate::test_env_lock();
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-grep-context-test");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("a.txt"), "aaa\nbbb\nneedle ccc\nddd\neee\n").unwrap();

        let input = json!({
            "pattern": "needle",
            "path": base.to_str().unwrap(),
            "context": 2
        })
        .to_string();
        let out = run_grep_search(&input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let ms = v["matches"].as_array().unwrap();
        // 2 before + the match + 2 after = 5 entries, lines 1..=5
        let lines: Vec<u64> = ms.iter().map(|m| m["line"].as_u64().unwrap()).collect();
        assert_eq!(lines, vec![1, 2, 3, 4, 5], "got: {out}");
        // only line 3 is the real match; the rest are context
        for m in ms {
            let expect = m["line"].as_u64().unwrap() == 3;
            assert_eq!(
                m["is_match"].as_bool().unwrap(),
                expect,
                "line {} flag wrong: {out}",
                m["line"]
            );
        }
        // match_count counts MATCH lines only, not the context lines
        assert_eq!(v["match_count"].as_u64().unwrap(), 1, "got: {out}");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn grep_search_context_dedupes_overlapping_windows() {
        let _eg = crate::test_env_lock();
        let base = user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-grep-context-overlap");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        // matches on lines 2 and 4; context=2 makes their windows overlap at line 3
        fs::write(base.join("a.txt"), "xxx\nneedle a\nyyy\nneedle b\nzzz\n").unwrap();

        let input = json!({
            "pattern": "needle",
            "path": base.to_str().unwrap(),
            "context": 2
        })
        .to_string();
        let out = run_grep_search(&input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        let ms = v["matches"].as_array().unwrap();
        let lines: Vec<u64> = ms.iter().map(|m| m["line"].as_u64().unwrap()).collect();
        // each of lines 1..=5 appears exactly once — the overlap isn't duplicated
        assert_eq!(
            lines,
            vec![1, 2, 3, 4, 5],
            "overlapping windows must not duplicate a line: {out}"
        );
        assert_eq!(v["match_count"].as_u64().unwrap(), 2, "got: {out}");
        let _ = fs::remove_dir_all(&base);
    }
}
