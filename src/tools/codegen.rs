//! Code-generation group — 2 tools (generate_code, spawn_agent) plus
//! the reference-file extraction infrastructure that generate_code
//! depends on.
//!
//! **Reference-file extraction** (Sprint 13 brownfield fix): when the
//! user asks generate_code to write tests/code that references an
//! existing file, we extract every path-like token from three sources
//! (priority order):
//!   1. Per-turn stash — paths the entry points pre-extracted from the
//!      raw user prompt (most reliable: bypasses the brain entirely).
//!   2. `reference_files` — the explicit schema param.
//!   3. `description` — fallback scan of the free-form description.
//!
//! The collector is conservative: it surfaces only files that (a)
//! syntactically look like a path, and (b) actually exist on disk
//! under the read policy. Size caps keep the coder prompt under ~70 KB.
//!
//! `set_current_turn_paths` and `extract_user_prompt_paths` stay `pub`
//! and are re-exported from the `tools` module — REPL / single-shot /
//! Telegram / TUI entry points call them before each turn.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use super::{ensure_dir, files_dir, validate_read_path, validate_write_path};

/// File extensions we'll include as reference context.
const REF_EXTENSIONS: &[&str] = &[
    "py", "rs", "js", "mjs", "cjs", "jsx", "ts", "tsx", "html", "htm", "css", "json", "toml",
    "yaml", "yml", "md", "txt", "sh", "bash", "go", "java", "c", "cpp", "cc", "cxx", "h", "hpp",
    "rb", "php", "sql", "xml", "ini", "cfg", "conf",
];

/// Max files, per-file byte cap, and total byte cap. Keeps the coder prompt
/// below ~70 KB even when the user references several modules.
const REF_MAX_FILES: usize = 4;
const REF_MAX_BYTES_PER_FILE: usize = 16 * 1024;
const REF_MAX_BYTES_TOTAL: usize = 64 * 1024;

// ────────────────────────────────────────────────────────────────────────────
// Per-turn user-prompt path stash (Sprint 13.2 — bypass-the-brain brownfield)
//
// The brain summarises the user prompt before constructing tool calls and
// regularly drops file paths. Even with the explicit `reference_files` schema
// param, the 4b brain rarely populates it. Solution: extract paths from the
// raw user prompt at the entry point (REPL / single-shot / Telegram / TUI),
// stash them here, and merge in `collect_reference_files`. Bypasses the brain
// entirely. Each entry point overwrites the stash before submitting the turn.
// ────────────────────────────────────────────────────────────────────────────

static CURRENT_TURN_PATHS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn current_turn_paths_mu() -> &'static Mutex<Vec<String>> {
    CURRENT_TURN_PATHS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Replace the per-turn path list. Called from each entry point with the paths
/// extracted from the raw user prompt. An empty Vec clears the stash, which is
/// the right thing to do for non-brownfield prompts (no leakage between turns).
pub fn set_current_turn_paths(paths: Vec<String>) {
    if let Ok(mut g) = current_turn_paths_mu().lock() {
        *g = paths;
    }
}

/// Read the current stash. Returns an empty Vec if poisoned (defensive — we'd
/// rather degrade to "no refs" than panic the agent loop).
pub(crate) fn current_turn_paths() -> Vec<String> {
    current_turn_paths_mu()
        .lock()
        .map(|g| g.clone())
        .unwrap_or_default()
}

/// Scan the raw user prompt for path tokens and keep only those that resolve
/// to an existing file under the read policy. Used by entry points to populate
/// the per-turn stash.
#[must_use]
pub fn extract_user_prompt_paths(prompt: &str) -> Vec<String> {
    extract_path_candidates(prompt)
        .into_iter()
        .filter(|t| resolve_reference(t).is_some())
        .collect()
}

/// Collect reference files for the coder prompt. Three sources, in priority order:
///   1. **Per-turn stash** — paths the system pre-extracted from the raw user
///      prompt (Sprint 13.2). Most reliable: bypasses the brain entirely.
///   2. `explicit` — paths the brain passed via the `reference_files` tool param.
///      Useful when the brain follows the schema instruction.
///   3. `description` — fallback path-scan for when the brain forgets BOTH the
///      param AND the path didn't make it into the user message verbatim.
///
/// All three go through the same `validate_read_path` policy and size caps,
/// and dedup by absolute path so a path hit on multiple sources only loads once.
pub(crate) fn collect_reference_files(
    explicit: &[&str],
    description: &str,
) -> Vec<crate::codet::ReferenceFile> {
    let mut out: Vec<crate::codet::ReferenceFile> = Vec::new();
    let mut seen_abs: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut total_bytes: usize = 0;

    let stash_iter = current_turn_paths().into_iter();
    let explicit_iter = explicit.iter().map(|s| (*s).to_string());
    let scanner_iter = extract_path_candidates(description).into_iter();
    for token in stash_iter.chain(explicit_iter).chain(scanner_iter) {
        if out.len() >= REF_MAX_FILES {
            break;
        }
        let Some(resolved) = resolve_reference(&token) else {
            continue;
        };
        if !seen_abs.insert(resolved.clone()) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&resolved) else {
            continue;
        };
        let trimmed = truncate_content(content);
        if total_bytes.saturating_add(trimmed.len()) > REF_MAX_BYTES_TOTAL {
            break;
        }
        total_bytes += trimmed.len();
        out.push(crate::codet::ReferenceFile {
            path: token,
            content: trimmed,
        });
    }
    out
}

fn truncate_content(mut content: String) -> String {
    if content.len() > REF_MAX_BYTES_PER_FILE {
        // Truncate at a char boundary, then annotate.
        let mut cut = REF_MAX_BYTES_PER_FILE;
        while cut > 0 && !content.is_char_boundary(cut) {
            cut -= 1;
        }
        content.truncate(cut);
        content.push_str("\n... [truncated — file continues]\n");
    }
    content
}

/// Break a free-form description into path-shaped candidate tokens, stripping
/// surrounding quotes/brackets/trailing punctuation. Does NOT check the
/// filesystem — `resolve_reference` does that.
fn extract_path_candidates(text: &str) -> Vec<String> {
    let mut raw: Vec<String> = Vec::new();
    let mut buf = String::new();
    for c in text.chars() {
        if c.is_whitespace()
            || matches!(
                c,
                ',' | ';' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\'' | '`' | '<' | '>'
            )
        {
            if !buf.is_empty() {
                raw.push(std::mem::take(&mut buf));
            }
        } else {
            buf.push(c);
        }
    }
    if !buf.is_empty() {
        raw.push(buf);
    }

    raw.into_iter()
        .filter_map(|t| {
            // Strip trailing sentence punctuation (em-dash, en-dash, etc).
            let trimmed = t
                .trim_end_matches(|c: char| {
                    matches!(c, '.' | ',' | ';' | ':' | '!' | '?' | '—' | '–' | ')')
                })
                .to_string();
            if trimmed.is_empty() {
                return None;
            }
            // URLs look like paths-with-extensions but aren't reachable via
            // the filesystem — drop them before they trip resolve_reference.
            if trimmed.contains("://") {
                return None;
            }
            if looks_like_path(&trimmed) || has_code_extension(&trimmed) {
                Some(trimmed)
            } else {
                None
            }
        })
        .collect()
}

/// `true` iff the token uses explicit path syntax (tilde, absolute, dotted
/// relative, or a Windows drive letter). URLs are excluded.
fn looks_like_path(s: &str) -> bool {
    if s.contains("://") {
        return false;
    }
    if s.starts_with("~/") || s.starts_with("~\\") {
        return true;
    }
    if s.starts_with("./") || s.starts_with(".\\") || s.starts_with("../") || s.starts_with("..\\")
    {
        return true;
    }
    if s.starts_with('/') || s.starts_with('\\') {
        return true;
    }
    let bytes = s.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/')
}

fn has_code_extension(s: &str) -> bool {
    Path::new(s)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            let lower = e.to_ascii_lowercase();
            REF_EXTENSIONS.contains(&lower.as_str())
        })
}

/// Resolve a token to an absolute path on disk, or `None` if no readable
/// file exists under $HOME, the scratch dir, or the current working dir.
fn resolve_reference(token: &str) -> Option<PathBuf> {
    // Explicit path: use the same read-policy as read_file.
    if looks_like_path(token) {
        return validate_read_path(token).ok().filter(|p| p.is_file());
    }
    // Bare filename with a code extension: try scratch dir then cwd.
    if !has_code_extension(token) {
        return None;
    }
    for dir in [
        files_dir(),
        std::env::current_dir().unwrap_or_else(|_| files_dir()),
    ] {
        let candidate = dir.join(token);
        if candidate.is_file() {
            let as_string = candidate.to_string_lossy().to_string();
            if let Ok(validated) = validate_read_path(&as_string) {
                return Some(validated);
            }
        }
    }
    None
}

pub(super) fn schemas() -> Vec<Value> {
    vec![
        json!({
            "type": "function",
            "function": {
                "name": "generate_code",
                "description": "Generate code using the specialized coder model and write it to a file. USE THIS instead of write_file for any code. Supports Python, Rust, JavaScript, TypeScript, HTML, CSS. Auto-validates syntax and tests. The file is written to disk; reply with a SHORT confirmation (path + 1 sentence). DO NOT paste the generated code in your reply — it bloats the conversation and the user can already open the file. BROWNFIELD: when the user mentions an existing file the new code must match (e.g. 'add tests for X.py', 'extend X.py', 'refactor X.py'), ALWAYS list those file paths in `reference_files` so the coder can read the real API instead of inventing one.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "description":     { "type": "string", "description": "What code to write — include language, functions, tests needed" },
                        "filename":        { "type": "string", "description": "Filename (e.g. 'calc.py', 'lib.rs', 'app.ts'). Extension sets the language." },
                        "reference_files": { "type": "array", "items": { "type": "string" }, "description": "Existing file paths the coder MUST read before writing (real class/method names, signatures, exceptions). Pass each path as the user typed it — '~/.claudette/files/X.py', './X.py', or 'X.py'. Up to 4 files; oversize files are auto-truncated." }
                    },
                    "required": ["description", "filename"]
                }
            }
        }),
        json!({
            "type": "function",
            "function": {
                "name": "spawn_agent",
                "description": "Delegate a task to a specialized agent. 'researcher' for web/file/code research, 'gitops' for git workflows, 'reviewer' for code review.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "agent_type": { "type": "string", "enum": ["researcher", "gitops", "reviewer"], "description": "Agent type" },
                        "task":       { "type": "string", "description": "Task description for the agent" },
                        "auto":       { "type": "boolean", "description": "Skip confirmation prompts for dangerous tools (default false)" }
                    },
                    "required": ["agent_type", "task"]
                }
            }
        }),
    ]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "generate_code" => run_generate_code(input),
        "spawn_agent" => run_spawn_agent(input),
        _ => return None,
    };
    Some(result)
}

fn run_generate_code(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("generate_code: invalid JSON ({e}): {input}"))?;
    let description = v
        .get("description")
        .and_then(Value::as_str)
        .ok_or("generate_code: missing 'description'")?;
    let filename = v
        .get("filename")
        .and_then(Value::as_str)
        .ok_or("generate_code: missing 'filename'")?;

    // Infer language from extension.
    let ext = Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("text");
    let language = match ext {
        "py" => "Python",
        "rs" => "Rust",
        "js" => "JavaScript",
        "ts" => "TypeScript",
        "php" => "PHP",
        "rb" => "Ruby",
        "go" => "Go",
        "java" => "Java",
        "c" | "h" => "C",
        "cpp" | "hpp" => "C++",
        "sh" | "bash" => "Bash",
        other => other,
    };

    // Collect reference files for the coder. Two signals:
    //   - `reference_files`: explicit array the brain passed (deterministic).
    //   - `description`: free-form scan for path tokens the brain mentioned in prose.
    // The explicit param is the contract; the scanner stays as a fallback so
    // brains that forget the param still get partial coverage.
    // Brownfield fix v2 (Sprint 13.1, 2026-04-18) — see project_sprint13_brownfield.
    let explicit_refs: Vec<&str> = v
        .get("reference_files")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let references = collect_reference_files(&explicit_refs, description);

    // Generate code via the coder model.
    let code = crate::codet::generate_code(description, language, &references)
        .ok_or("generate_code: coder model returned no usable output")?;

    // Write via the same sandbox logic as write_file (bare relative paths
    // resolve under ~/.claudette/files/).
    let resolved_input = if Path::new(filename).is_absolute()
        || filename.starts_with("~/")
        || filename.starts_with("~\\")
    {
        filename.to_string()
    } else {
        files_dir().join(filename).display().to_string()
    };
    let path = validate_write_path(&resolved_input)?;

    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    fs::write(&path, &code)
        .map_err(|e| format!("generate_code: write {} failed: {e}", path.display()))?;

    let mut result = json!({
        "ok": true,
        "path": path.display().to_string(),
        "bytes": code.len(),
        "language": language,
        "generated_by": crate::codet::coder_model(),
        // Strong hint for the model: the file is on disk, do not paste
        "reply_hint": "File written. Reply with: file path + 1-sentence \
                       summary. DO NOT include the code in your response.",
    });

    // Run Codet validation (same as write_file post-write hook). Pass the
    // references so the fix-loop also sees the real API when repairing tests.
    if let Some(validation) = crate::codet::validate_code_file(&path, &references) {
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

fn run_spawn_agent(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("spawn_agent: invalid JSON ({e}): {input}"))?;
    let type_str = v
        .get("agent_type")
        .and_then(Value::as_str)
        .ok_or("spawn_agent: missing 'agent_type'")?;
    let agent_type = crate::agents::AgentType::parse(type_str).ok_or_else(|| {
        format!("spawn_agent: unknown agent type '{type_str}'. Use 'researcher' or 'gitops'.")
    })?;
    let task = v
        .get("task")
        .and_then(Value::as_str)
        .ok_or("spawn_agent: missing 'task'")?;
    let auto_mode = v.get("auto").and_then(Value::as_bool).unwrap_or(false);

    crate::agents::spawn_agent(agent_type, task, auto_mode)
}

#[cfg(test)]
mod tests {
    use super::super::user_home;
    use super::*;

    /// Serializer for any test that reads or writes `CURRENT_TURN_PATHS`.
    /// Cargo runs tests in parallel; without this guard, a stash-setting test
    /// can leak state into a stash-reading test running concurrently.
    static STASH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_stash() -> std::sync::MutexGuard<'static, ()> {
        // Recover from poisoning — a panic in one test must not block the rest.
        STASH_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[test]
    fn looks_like_path_recognises_common_shapes() {
        assert!(looks_like_path("~/foo/bar.py"));
        assert!(looks_like_path("~\\foo\\bar.py"));
        assert!(looks_like_path("./foo"));
        assert!(looks_like_path("../foo"));
        assert!(looks_like_path("/abs/path"));
        assert!(looks_like_path("C:\\Users\\me\\x.py"));
        assert!(looks_like_path("D:/dev/claudette/x.py"));
        assert!(!looks_like_path("plainword"));
        assert!(!looks_like_path("file.py")); // bare filename — not a path per se
        assert!(!looks_like_path("https://example.com/x.py"));
        assert!(!looks_like_path("http://example.com/x.py"));
    }

    #[test]
    fn has_code_extension_recognises_code_files() {
        assert!(has_code_extension("calculator.py"));
        assert!(has_code_extension("lib.RS")); // case-insensitive
        assert!(has_code_extension("path/to/file.ts"));
        assert!(!has_code_extension("no-extension"));
        assert!(!has_code_extension("readme"));
        // Extensions we don't include shouldn't leak in.
        assert!(!has_code_extension("archive.zip"));
    }

    #[test]
    fn extract_path_candidates_strips_punctuation_and_brackets() {
        let text = "Read the file ~/.claudette/files/calculator.py — it's a module.";
        let cands = extract_path_candidates(text);
        assert!(
            cands
                .iter()
                .any(|t| t == "~/.claudette/files/calculator.py"),
            "missing tilde path, got: {cands:?}",
        );
    }

    #[test]
    fn extract_path_candidates_keeps_bare_code_filename() {
        let cands = extract_path_candidates("Please read calculator.py carefully.");
        assert!(
            cands.iter().any(|t| t == "calculator.py"),
            "missing bare filename, got: {cands:?}",
        );
    }

    #[test]
    fn extract_path_candidates_ignores_urls_and_prose() {
        let cands =
            extract_path_candidates("Visit https://example.com/x.py then write a greeting.");
        // No URL, no plain prose words.
        assert!(
            !cands.iter().any(|t| t.contains("example.com")),
            "leaked URL: {cands:?}",
        );
        assert!(
            !cands.iter().any(|t| t == "greeting"),
            "kept prose word: {cands:?}",
        );
    }

    #[test]
    fn collect_reference_files_reads_tilde_path() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]); // start clean
                                        // Write a fixture under the user's home so validate_read_path accepts it.
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_fixture.py");
        let body = "class RefFixture:\n    def hello(self):\n        return 'hi'\n";
        fs::write(&fixture, body).unwrap();

        let desc =
            "Read the file ~/.claudette/files/refsprint_fixture.py and write tests for its API."
                .to_string();
        let refs = collect_reference_files(&[], &desc);

        // Cleanup before asserting so we don't leak fixtures on failure.
        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "expected 1 reference, got {}", refs.len());
        assert!(
            refs[0].content.contains("class RefFixture"),
            "content missing, got: {:?}",
            refs[0].content
        );
        assert_eq!(refs[0].path, "~/.claudette/files/refsprint_fixture.py");
    }

    #[test]
    fn collect_reference_files_ignores_missing_and_non_code() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        // A description with a URL, a word, and a nonexistent filename.
        let desc = "Write a function. No file here. See http://example.com/foo.py and ghost.py.";
        let refs = collect_reference_files(&[], desc);
        assert!(
            refs.is_empty(),
            "expected no refs for missing files, got {refs:?}",
        );
    }

    #[test]
    fn collect_reference_files_caps_file_size() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_big_fixture.py");
        // 20 KB of Python, over the 16 KB per-file cap.
        let body: String = "x = 1\n".repeat(20 * 1024 / 6 + 1);
        fs::write(&fixture, &body).unwrap();

        let desc = "See ~/.claudette/files/refsprint_big_fixture.py".to_string();
        let refs = collect_reference_files(&[], &desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1);
        assert!(
            refs[0].content.contains("[truncated — file continues]"),
            "missing truncation marker",
        );
        assert!(
            refs[0].content.len() <= 16 * 1024 + 100,
            "content not truncated: {} bytes",
            refs[0].content.len()
        );
    }

    // ─── Sprint 13.1 — explicit reference_files param ────────────────

    #[test]
    fn collect_reference_files_uses_explicit_param() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_explicit_fixture.py");
        let body = "def explicit_marker():\n    return 'from explicit param'\n";
        fs::write(&fixture, body).unwrap();

        // Description has NO path tokens — only the explicit param does.
        let desc = "Write tests for the helper module.";
        let explicit = ["~/.claudette/files/refsprint_explicit_fixture.py"];
        let refs = collect_reference_files(&explicit, desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "expected 1 reference, got {}", refs.len());
        assert!(
            refs[0].content.contains("explicit_marker"),
            "content missing, got: {:?}",
            refs[0].content
        );
    }

    #[test]
    fn collect_reference_files_dedups_explicit_and_scanner() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_dedup_fixture.py");
        fs::write(&fixture, "x = 1\n").unwrap();

        // Same path appears in BOTH the explicit param and the description text.
        let desc = "Read ~/.claudette/files/refsprint_dedup_fixture.py and tests.";
        let explicit = ["~/.claudette/files/refsprint_dedup_fixture.py"];
        let refs = collect_reference_files(&explicit, desc);

        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "duplicate not collapsed: {refs:?}");
    }

    #[test]
    fn collect_reference_files_silently_drops_invalid_explicit_paths() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        // Explicit paths that don't exist on disk are filtered out, not erroring.
        let explicit = ["/this/path/does/not/exist.py", "~/no_such_file.py"];
        let refs = collect_reference_files(&explicit, "irrelevant description");
        assert!(refs.is_empty(), "expected empty, got {refs:?}");
    }

    // ─── Sprint 13.2 — per-turn user-prompt path stash ───────────────

    #[test]
    fn extract_user_prompt_paths_keeps_existing_files_only() {
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_stash_real.py");
        fs::write(&fixture, "x = 1\n").unwrap();

        let prompt = "Add tests for ~/.claudette/files/refsprint_stash_real.py \
                      and also for ~/.claudette/files/refsprint_stash_ghost.py";
        let paths = extract_user_prompt_paths(prompt);
        let _ = fs::remove_file(&fixture);

        assert!(
            paths.iter().any(|p| p.contains("refsprint_stash_real.py")),
            "real path missing: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.contains("refsprint_stash_ghost.py")),
            "ghost path leaked: {paths:?}"
        );
    }

    #[test]
    fn collect_reference_files_honours_turn_stash() {
        let _g = lock_stash();
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("refsprint_stash_fixture.py");
        let body = "def stash_marker():\n    return 'from turn stash'\n";
        fs::write(&fixture, body).unwrap();

        // Stash one path; pass empty explicit, irrelevant description.
        set_current_turn_paths(vec![
            "~/.claudette/files/refsprint_stash_fixture.py".to_string()
        ]);
        let refs = collect_reference_files(&[], "Write tests for the helper.");

        // Always clear the stash so other tests aren't affected.
        set_current_turn_paths(vec![]);
        let _ = fs::remove_file(&fixture);

        assert_eq!(refs.len(), 1, "stash not honoured: {refs:?}");
        assert!(
            refs[0].content.contains("stash_marker"),
            "wrong content: {:?}",
            refs[0].content
        );
    }

    #[test]
    fn set_current_turn_paths_overwrites_previous_stash() {
        let _g = lock_stash();
        set_current_turn_paths(vec!["a.py".to_string(), "b.py".to_string()]);
        assert_eq!(current_turn_paths().len(), 2);
        set_current_turn_paths(vec!["c.py".to_string()]);
        assert_eq!(current_turn_paths(), vec!["c.py".to_string()]);
        set_current_turn_paths(vec![]);
        assert!(current_turn_paths().is_empty());
    }

    #[test]
    fn collect_reference_files_explicit_respects_max_files() {
        let _g = lock_stash();
        set_current_turn_paths(vec![]);
        let dir = user_home().join(".claudette").join("files");
        fs::create_dir_all(&dir).unwrap();
        let mut fixtures = Vec::new();
        let mut explicit_paths = Vec::new();
        for i in 0..6 {
            let p = dir.join(format!("refsprint_cap_fixture_{i}.py"));
            fs::write(&p, format!("# fixture {i}\nx = {i}\n")).unwrap();
            fixtures.push(p);
            explicit_paths.push(format!("~/.claudette/files/refsprint_cap_fixture_{i}.py"));
        }
        let explicit_refs: Vec<&str> = explicit_paths.iter().map(String::as_str).collect();

        let refs = collect_reference_files(&explicit_refs, "");

        for f in &fixtures {
            let _ = fs::remove_file(f);
        }

        assert_eq!(
            refs.len(),
            REF_MAX_FILES,
            "expected cap, got {}",
            refs.len()
        );
    }

    #[test]
    fn schemas_lists_two_tools() {
        let schemas = schemas();
        assert_eq!(schemas.len(), 2);
        let names: Vec<&str> = schemas
            .iter()
            .filter_map(|v| v.pointer("/function/name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, ["generate_code", "spawn_agent"]);
    }
}
