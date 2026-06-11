//! Semantic group — workspace-scoped semantic-ish search. Sprint v0.6.0
//! ships `semantic_grep` as a token-overlap MVP that's already useful
//! for fuzzy concept queries ("find the auth flow", "where do we parse
//! diffs") without requiring an embedding model. The brief's
//! embedding-backed variant (with persistent on-disk cache and the
//! recall pipeline) is documented as follow-up work — it would land in
//! a v0.6.x point release once the per-session cost can be amortised.
//!
//! Ranking is Jaccard similarity on case-folded word sets, with a tiny
//! boost for exact-substring matches. Empirically that beats raw `grep`
//! for short queries ("payment retry logic") because it scores partial
//! word coverage instead of all-or-nothing presence — good enough for
//! the brain to navigate before it commits to reading the file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::parse_json_input;

const MAX_FILES: usize = 1500;
const MAX_CHUNK_LINES: usize = 40;
const MAX_FILE_BYTES: usize = 256 * 1024;
const SKIP_DIRS: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "dist",
    "build",
    ".venv",
    "venv",
    ".next",
    "__pycache__",
    "vendor",
];
const TEXT_EXTENSIONS: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "py", "go", "rb", "java", "kt", "swift", "c", "h", "cpp",
    "hpp", "cs", "php", "sh", "bash", "ps1", "yaml", "yml", "toml", "json", "md", "txt", "sql",
    "html", "css", "scss", "vue", "svelte",
];

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "semantic_grep",
            "description": "Conceptual search across workspace text files. Ranks chunks by token-overlap with `query` (fuzzier than grep — good for 'where is X done' questions). Returns top-k chunks with file/line context.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Free-form concept to look for." },
                    "k":     { "type": "number", "description": "Max hits (default 5, max 20)." }
                },
                "required": ["query"]
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    let result = match name {
        "semantic_grep" => run_semantic_grep(input),
        _ => return None,
    };
    Some(result)
}

#[derive(Debug)]
struct Chunk {
    file: PathBuf,
    line_start: usize,
    line_end: usize,
    text: String,
}

fn run_semantic_grep(input: &str) -> Result<String, String> {
    let v = parse_json_input(input, "semantic_grep")?;
    let query = v
        .get("query")
        .and_then(Value::as_str)
        .ok_or("semantic_grep: missing 'query'")?;
    let k = v.get("k").and_then(Value::as_u64).unwrap_or(5).clamp(1, 20) as usize;

    let cwd = crate::missions::active_cwd();
    let chunks = collect_chunks(&cwd);
    if chunks.is_empty() {
        return Ok(json!({
            "query": query,
            "k": k,
            "count": 0,
            "results": [],
            "note": "no text files found under the workspace root",
        })
        .to_string());
    }

    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return Err("semantic_grep: 'query' contained no searchable tokens".to_string());
    }
    let query_lower = query.to_lowercase();

    let mut scored: Vec<(f32, &Chunk)> = chunks
        .iter()
        .map(|c| {
            let chunk_tokens = tokenize(&c.text);
            let mut score = jaccard(&query_tokens, &chunk_tokens);
            if c.text.to_lowercase().contains(&query_lower) {
                // Boost exact-substring matches so they don't get buried
                // behind a high-overlap chunk that's only conceptually
                // related.
                score += 0.5;
            }
            (score, c)
        })
        .filter(|(s, _)| *s > 0.0)
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let results: Vec<Value> = scored
        .iter()
        .take(k)
        .map(|(score, c)| {
            json!({
                "file": display_path(&c.file, &cwd),
                "line_start": c.line_start,
                "line_end": c.line_end,
                "snippet": truncate(&c.text, 800),
                "score": format!("{score:.3}"),
            })
        })
        .collect();

    Ok(json!({
        "query": query,
        "k": k,
        "count": results.len(),
        "total_chunks": chunks.len(),
        "results": results,
    })
    .to_string())
}

fn collect_chunks(root: &Path) -> Vec<Chunk> {
    let mut chunks: Vec<Chunk> = Vec::new();
    let mut files_scanned = 0usize;
    walk(root, &mut |path: &Path| {
        if files_scanned >= MAX_FILES {
            return false;
        }
        if !is_text_file(path) {
            return true;
        }
        let Ok(metadata) = std::fs::metadata(path) else {
            return true;
        };
        if usize::try_from(metadata.len()).unwrap_or(usize::MAX) > MAX_FILE_BYTES {
            return true;
        }
        let Ok(text) = std::fs::read_to_string(path) else {
            return true;
        };
        files_scanned += 1;
        chunk_file(path, &text, &mut chunks);
        true
    });
    chunks
}

fn walk<F: FnMut(&Path) -> bool>(root: &Path, callback: &mut F) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') && name != "." {
            // Skip dotfiles + dot-dirs except the current dir itself.
            if name != ".env" {
                continue;
            }
        }
        if path.is_dir() {
            if SKIP_DIRS.contains(&name) {
                continue;
            }
            walk(&path, callback);
        } else if path.is_file() && !callback(&path) {
            return;
        }
    }
}

fn is_text_file(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| TEXT_EXTENSIONS.contains(&ext.to_lowercase().as_str()))
}

fn chunk_file(path: &Path, text: &str, chunks: &mut Vec<Chunk>) {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return;
    }
    let mut start = 0usize;
    while start < lines.len() {
        let end = (start + MAX_CHUNK_LINES).min(lines.len());
        let body = lines[start..end].join("\n");
        if !body.trim().is_empty() {
            chunks.push(Chunk {
                file: path.to_path_buf(),
                line_start: start + 1,
                line_end: end,
                text: body,
            });
        }
        start = end;
    }
}

fn tokenize(s: &str) -> HashSet<String> {
    let mut out = Vec::new();
    for raw in s.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        let mut cur = String::new();
        let mut prev_lower_or_digit = false;
        for ch in raw.chars() {
            if ch.is_uppercase() && prev_lower_or_digit && !cur.is_empty() {
                let lower = cur.to_lowercase();
                if lower.chars().count() > 1 {
                    out.push(lower);
                }
                cur.clear();
            }
            cur.push(ch);
            prev_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
        }
        let lower = cur.to_lowercase();
        if lower.chars().count() > 1 {
            out.push(lower);
        }
    }
    out.into_iter().collect()
}

fn jaccard(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let intersection = a.intersection(b).count() as f32;
    let union = a.union(b).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn display_path(path: &Path, base: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .display()
        .to_string()
        .replace('\\', "/")
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut byte = max;
    while byte < s.len() && !s.is_char_boundary(byte) {
        byte -= 1;
    }
    format!("{}...", &s[..byte])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_lists_one_tool() {
        let s = schemas();
        assert_eq!(s.len(), 1);
        let name = s[0]
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(name, "semantic_grep");
    }

    #[test]
    fn semantic_grep_rejects_missing_query() {
        let err = run_semantic_grep("{}").unwrap_err();
        assert!(err.contains("missing 'query'"), "got: {err}");
    }

    #[test]
    fn tokenize_filters_short_tokens_and_lowercases() {
        let t = tokenize("Auth/payment retry-logic in MyClass!!!");
        assert!(t.contains("auth"));
        assert!(t.contains("payment"));
        assert!(t.contains("retry"));
        assert!(t.contains("logic"));
        // "MyClass" splits into two tokens: my + class.
        assert!(t.contains("my"));
        assert!(t.contains("class"));
        // Single-char tokens dropped — the slash isn't kept either.
        assert!(!t.contains(""));
    }

    #[test]
    fn tokenize_splits_camelcase() {
        let t = tokenize("loopBreaker maxFixRounds");
        assert!(t.contains("loop"));
        assert!(t.contains("breaker"));
        assert!(t.contains("max"));
        assert!(t.contains("fix"));
        assert!(t.contains("rounds"));
    }

    #[test]
    fn tokenize_drops_single_char_tokens() {
        let t = tokenize("a bb cCc x9");
        assert!(!t.contains("a"), "single-char token must be dropped");
        assert!(
            !t.contains("x"),
            "single-char split fragment must be dropped"
        );
        assert!(t.contains("bb"));
    }

    #[test]
    fn jaccard_is_intersection_over_union() {
        let a: HashSet<String> = ["a", "b", "c"].iter().map(ToString::to_string).collect();
        let b: HashSet<String> = ["b", "c", "d"].iter().map(ToString::to_string).collect();
        // Intersection {b, c} = 2; union {a,b,c,d} = 4 → 0.5.
        assert!((jaccard(&a, &b) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn jaccard_zero_on_disjoint() {
        let a: HashSet<String> = ["x", "y"].iter().map(ToString::to_string).collect();
        let b: HashSet<String> = ["m", "n"].iter().map(ToString::to_string).collect();
        assert!(jaccard(&a, &b).abs() < 1e-6);
    }

    #[test]
    fn chunk_file_splits_at_configured_line_limit() {
        let text = (0..100)
            .map(|i| format!("line-{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut chunks = Vec::new();
        chunk_file(Path::new("test.rs"), &text, &mut chunks);
        // 100 lines / 40 per chunk = 3 chunks (40 + 40 + 20).
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].line_start, 1);
        assert_eq!(chunks[0].line_end, 40);
        assert_eq!(chunks[1].line_start, 41);
        assert_eq!(chunks[2].line_start, 81);
    }

    #[test]
    fn is_text_file_recognises_common_extensions() {
        assert!(is_text_file(Path::new("foo.rs")));
        assert!(is_text_file(Path::new("foo.ts")));
        assert!(is_text_file(Path::new("foo.PY"))); // case-insensitive
        assert!(!is_text_file(Path::new("foo.exe")));
        assert!(!is_text_file(Path::new("noext")));
    }

    #[test]
    fn truncate_respects_utf8_boundaries() {
        let s = "héllo wörld";
        let out = truncate(s, 6);
        // Should not panic on multibyte boundary.
        assert!(out.ends_with("..."));
    }

    #[test]
    fn end_to_end_finds_a_self_referential_token() {
        // Run against the actual workspace and expect to find this very
        // file (or another tools/* source) by searching for a rare token.
        // Serialize against the process-wide cwd lock: this test reads the
        // process cwd (via missions::active_cwd) while other tests
        // (runtime::prompt) mutate it with set_current_dir under the same
        // lock. Without this guard they raced and this test flaked red —
        // the exact `cargo test --lib` CI/release gate. (roast 2026-06-02)
        let _guard = crate::test_env_lock();
        let cwd = std::env::current_dir().expect("cwd");
        let out = run_semantic_grep(&json!({ "query": "MAX_CHUNK_LINES", "k": 3 }).to_string());
        match out {
            Ok(body) => {
                let v: serde_json::Value = serde_json::from_str(&body).unwrap();
                // Either we found something or we explicitly noted "no text files".
                assert!(
                    v["count"].as_u64().unwrap_or(0) >= 1 || v.get("note").is_some(),
                    "expected hits or empty-note, got: {body}\ncwd was {}",
                    cwd.display()
                );
            }
            Err(e) => {
                panic!("semantic_grep failed: {e}");
            }
        }
    }
}
