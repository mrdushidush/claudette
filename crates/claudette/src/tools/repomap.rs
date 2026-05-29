//! repo_map — Aider-style ranked symbol outline of the workspace.
//!
//! Small coding brains localize by *guessing* grep patterns; when they don't
//! know the exact symbol they spiral (observed on qwen3.6-35b q3: a "where is
//! X configured" query burned the whole iteration budget guessing regexes).
//! `repo_map` hands the brain a map instead. For a natural-language query it:
//!   1. walks the workspace gitignore-aware (ripgrep's `ignore` crate, same as
//!      grep_search) so build/dep/VCS dirs and *.log noise are skipped,
//!   2. extracts top-level definitions per file via language-aware patterns
//!      (rust / python / js-ts / go) with line numbers + signature snippets,
//!   3. ranks files by how well their symbol names + path match the query
//!      tokens, and returns the top files with their best symbols.
//!
//! One call replaces N grep guesses, and the signature snippet frequently
//! carries the answer itself (`const DEFAULT_MAX_FIX_ROUNDS: u32 = 3;`) so the
//! brain can answer a "what's the default" question without a follow-up read.

use std::path::Path;

use regex::Regex;
use serde_json::{json, Value};

use super::{validate_read_path, MAX_FILE_BYTES};

const MAX_FILES_SCANNED: usize = 5000;
const MAX_RESULT_FILES: usize = 15;
const MAX_SYMBOLS_PER_FILE: usize = 40;
const MAX_SIG_CHARS: usize = 160;

pub(super) fn schemas() -> Vec<Value> {
    vec![json!({
        "type": "function",
        "function": {
            "name": "repo_map",
            "description": "Localize code by concept. Returns a ranked outline of the workspace: the files whose top-level definitions (functions, types, constants) best match `query`, each with line numbers + signature snippets. Use this FIRST to find where something lives instead of guessing grep patterns — the snippet often shows the value/signature directly. Then read_file the cited line if you need more. (Languages: Rust, Python, JS/TS, Go.)",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What you're looking for, in words or symbol fragments (e.g. 'forge fix loop max rounds default')" },
                    "path":  { "type": "string", "description": "Directory to map (default: the workspace/project root)" }
                },
                "required": ["query"]
            }
        }
    })]
}

pub(super) fn dispatch(name: &str, input: &str) -> Option<Result<String, String>> {
    match name {
        "repo_map" => Some(run_repo_map(input)),
        _ => None,
    }
}

struct Symbol {
    line: usize,
    kind: &'static str,
    name: String,
    sig: String,
}

fn run_repo_map(input: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(input)
        .map_err(|e| format!("repo_map: invalid JSON ({e}): {input}"))?;
    let query = v
        .get("query")
        .and_then(Value::as_str)
        .ok_or("repo_map: missing 'query'")?;
    if query.trim().is_empty() {
        return Err("repo_map: query is empty".to_string());
    }
    // Default root: active mission tree → workspace cwd → first workspace root
    // → $HOME (mirrors grep_search's resolution).
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
    if !root.is_dir() {
        return Err(format!("repo_map: {} is not a directory", root.display()));
    }

    let query_tokens = tokenize(query);

    // (file, score, symbols) accumulated across the walk.
    let mut scored: Vec<(String, usize, Vec<Symbol>)> = Vec::new();
    let mut files_scanned = 0usize;
    let mut truncated = false;

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
                if super::SEARCH_SKIP_DIRS.contains(&name.as_ref()) {
                    return false;
                }
            }
            true
        })
        .build();

    for result in walker {
        let Ok(entry) = result else { continue };
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        if files_scanned >= MAX_FILES_SCANNED {
            truncated = true;
            break;
        }
        let p = entry.path();
        let Some(patterns) = patterns_for(p) else {
            continue;
        };
        files_scanned += 1;
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES as u64 {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(p) else {
            continue;
        };

        let path_tokens = tokenize(&p.to_string_lossy());
        let mut symbols: Vec<(usize, Symbol)> = Vec::new(); // (sym_score, symbol)
        let mut file_score = 0usize;

        for (lineno, line) in content.lines().enumerate() {
            for (kind, re) in &patterns {
                if let Some(caps) = re.captures(line) {
                    if let Some(name) = caps.get(1).map(|m| m.as_str()) {
                        let name_tokens = tokenize(name);
                        let sym_score = query_tokens
                            .iter()
                            .filter(|qt| name_tokens.iter().any(|nt| nt == *qt))
                            .count();
                        file_score += sym_score * 2;
                        symbols.push((
                            sym_score,
                            Symbol {
                                line: lineno + 1,
                                kind,
                                name: name.to_string(),
                                sig: line.trim().chars().take(MAX_SIG_CHARS).collect(),
                            },
                        ));
                        break; // one kind per line
                    }
                }
            }
        }

        // Path-token overlap (a query mentioning "search" should surface
        // search.rs even if no symbol name matches).
        file_score += query_tokens
            .iter()
            .filter(|qt| path_tokens.iter().any(|pt| pt == *qt))
            .count();

        if file_score == 0 || symbols.is_empty() {
            continue;
        }
        // Best-scoring symbols first, then source order; cap per file.
        symbols.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.line.cmp(&b.1.line)));
        let kept: Vec<Symbol> = symbols
            .into_iter()
            .take(MAX_SYMBOLS_PER_FILE)
            .map(|(_, s)| s)
            .collect();
        scored.push((p.display().to_string(), file_score, kept));
    }

    scored.sort_by_key(|f| std::cmp::Reverse(f.1));
    let result_files = scored.len().min(MAX_RESULT_FILES);
    let files_json: Vec<Value> = scored
        .into_iter()
        .take(MAX_RESULT_FILES)
        .map(|(file, score, syms)| {
            json!({
                "file": file,
                "score": score,
                "symbols": syms.iter().map(|s| json!({
                    "line": s.line,
                    "kind": s.kind,
                    "name": s.name,
                    "sig": s.sig,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    Ok(json!({
        "query": query,
        "root": root.display().to_string(),
        "files_scanned": files_scanned,
        "result_files": result_files,
        "truncated": truncated,
        "files": files_json,
    })
    .to_string())
}

/// Split an identifier or query into lowercase word tokens: breaks on
/// non-alphanumerics (so `snake_case` and `kebab-case` split) and on
/// camelCase boundaries (`maxFixRounds` → max, fix, rounds). Drops 1-char
/// tokens and a tiny stoplist of NL filler so "where is the X" matches X.
fn tokenize(s: &str) -> Vec<String> {
    const STOP: &[&str] = &[
        "the", "is", "are", "of", "in", "to", "where", "what", "how", "does", "do", "it", "this",
        "that", "for", "and", "or", "a", "an", "be", "on", "at",
    ];
    let mut out = Vec::new();
    for raw in s.split(|c: char| !c.is_alphanumeric()) {
        if raw.is_empty() {
            continue;
        }
        let mut cur = String::new();
        let mut prev_lower_or_digit = false;
        for ch in raw.chars() {
            if ch.is_uppercase() && prev_lower_or_digit && !cur.is_empty() {
                push_token(&mut out, &cur, STOP);
                cur.clear();
            }
            cur.push(ch);
            prev_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
        }
        push_token(&mut out, &cur, STOP);
    }
    out
}

fn push_token(out: &mut Vec<String>, tok: &str, stop: &[&str]) {
    if tok.chars().count() < 2 {
        return;
    }
    let lower = tok.to_lowercase();
    if stop.contains(&lower.as_str()) {
        return;
    }
    out.push(lower);
}

/// Language-aware definition patterns for a file, or `None` if the extension
/// isn't a supported source language. Each `Regex` captures the symbol NAME in
/// group 1. Compiled per call (cheap — a few dozen small patterns, and
/// repo_map runs rarely).
fn patterns_for(path: &Path) -> Option<Vec<(&'static str, Regex)>> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let pats: Vec<(&'static str, &str)> = match ext.as_str() {
        "rs" => vec![
            (
                "fn",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+)?fn\s+([A-Za-z_]\w*)",
            ),
            (
                "struct",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_]\w*)",
            ),
            (
                "enum",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?enum\s+([A-Za-z_]\w*)",
            ),
            (
                "trait",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?trait\s+([A-Za-z_]\w*)",
            ),
            (
                "type",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?type\s+([A-Za-z_]\w*)",
            ),
            (
                "const",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?const\s+([A-Za-z_]\w*)",
            ),
            (
                "static",
                r"^\s*(?:pub(?:\([^)]*\))?\s+)?static\s+(?:mut\s+)?([A-Za-z_]\w*)",
            ),
            ("mod", r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z_]\w*)"),
            ("macro", r"^\s*macro_rules!\s+([A-Za-z_]\w*)"),
        ],
        "py" => vec![
            ("def", r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)"),
            ("class", r"^\s*class\s+([A-Za-z_]\w*)"),
        ],
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "tsx" => vec![
            (
                "function",
                r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*\*?\s+([A-Za-z_$][\w$]*)",
            ),
            (
                "class",
                r"^\s*(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+([A-Za-z_$][\w$]*)",
            ),
            (
                "type",
                r"^\s*(?:export\s+)?(?:interface|type|enum)\s+([A-Za-z_$][\w$]*)",
            ),
            (
                "const",
                r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=",
            ),
        ],
        "go" => vec![
            ("func", r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)"),
            ("type", r"^\s*type\s+([A-Za-z_]\w*)"),
            ("const", r"^\s*(?:const|var)\s+([A-Za-z_]\w*)"),
        ],
        _ => return None,
    };
    Some(
        pats.into_iter()
            .filter_map(|(kind, p)| Regex::new(p).ok().map(|re| (kind, re)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_snake_and_camel_and_drops_filler() {
        assert_eq!(tokenize("max_fix_rounds"), ["max", "fix", "rounds"]);
        assert_eq!(tokenize("maxFixRounds"), ["max", "fix", "rounds"]);
        assert_eq!(
            tokenize("DEFAULT_MAX_FIX_ROUNDS"),
            ["default", "max", "fix", "rounds"]
        );
        // NL query: filler dropped, content kept.
        assert_eq!(
            tokenize("where is the fix loop max rounds default"),
            ["fix", "loop", "max", "rounds", "default"]
        );
    }

    #[test]
    fn rust_patterns_capture_fn_and_const_with_value() {
        let pats = patterns_for(Path::new("x.rs")).unwrap();
        let line_fn = "fn max_fix_rounds() -> u32 {";
        let line_const = "const DEFAULT_MAX_FIX_ROUNDS: u32 = 3;";
        let hit_fn = pats
            .iter()
            .find_map(|(k, re)| re.captures(line_fn).map(|c| (*k, c[1].to_string())));
        let hit_const = pats
            .iter()
            .find_map(|(k, re)| re.captures(line_const).map(|c| (*k, c[1].to_string())));
        assert_eq!(hit_fn, Some(("fn", "max_fix_rounds".to_string())));
        assert_eq!(
            hit_const,
            Some(("const", "DEFAULT_MAX_FIX_ROUNDS".to_string()))
        );
    }

    #[test]
    fn repo_map_ranks_the_matching_file_first_with_value_in_sig() {
        let base = super::super::user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-repomap-test-k3");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(
            base.join("src").join("run.rs"),
            "const DEFAULT_MAX_FIX_ROUNDS: u32 = 3;\nfn max_fix_rounds() -> u32 { 3 }\n",
        )
        .unwrap();
        std::fs::write(
            base.join("src").join("notes.rs"),
            "fn note_create() {}\nstruct Note {}\n",
        )
        .unwrap();

        let input = json!({
            "query": "forge fix loop max rounds default",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_repo_map(&input).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();

        let first = &v["files"][0];
        assert!(
            first["file"]
                .as_str()
                .unwrap()
                .replace('\\', "/")
                .contains("/src/run.rs"),
            "run.rs should rank first: {out}"
        );
        // The const's sig snippet carries the answer (= 3) directly.
        let sigs: String = first["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["sig"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(sigs.contains("= 3"), "expected the value in a sig: {sigs}");

        let _ = std::fs::remove_dir_all(&base);
    }
}
