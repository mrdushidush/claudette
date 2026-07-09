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
//!
//! `mode="refs"` is the exhaustive counterpart: a literal substring scan over
//! every readable file (not just the 4 definition languages) that returns a
//! deduped `distinct_names` enumeration plus source-first / docs-last hit
//! lists — for "list everywhere X is used" and "what's the real value past
//! the stale docs". See [`run_repo_refs`].

use std::fmt::Write;
use std::path::Path;

use regex::Regex;
use serde_json::{json, Value};

use super::{validate_read_path, MAX_FILE_BYTES};

const MAX_FILES_SCANNED: usize = 5000;
const MAX_RESULT_FILES: usize = 12;
const MAX_SYMBOLS_PER_FILE: usize = 20;
const MAX_SIG_CHARS: usize = 160;
/// `mode="refs"`: cap on individual hit lines returned (source + doc
/// combined). Higher than grep's 100 because the per-hit payload is just
/// file/line/snippet. `distinct_names` is computed over ALL hits before
/// this cap, so the enumeration answer is never truncated.
const MAX_REF_HITS: usize = 120;
/// `mode="refs"`: cap on the deduped identifier list (the enumeration
/// answer). 200 distinct matches of one needle is already pathological.
const MAX_DISTINCT_NAMES: usize = 200;

/// Source-code extensions for the refs-mode source/doc split. A hit in one
/// of these is `source` (authoritative); everything else (`.md`, `.txt`,
/// `.toml`, `.json`, `.yaml`, …) is `doc` and ranked below — so the brain
/// sees the real `const X = 3;` ahead of a stale doc saying `2`.
const SOURCE_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "mjs", "cjs", "jsx", "ts", "tsx", "go", "java", "c", "cc", "cpp", "cxx", "h",
    "hpp", "hh", "rb", "php", "sh", "bash", "sql", "kt", "kts", "swift", "scala", "cs",
];

fn is_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SOURCE_EXTENSIONS.iter().any(|s| e.eq_ignore_ascii_case(s)))
}

pub(super) fn schemas() -> Vec<Value> {
    // The "(Languages: …)" list is generated from LANG_PATTERNS so adding a
    // language stays a one-place edit (the table) — the schema follows.
    let description = format!(
        "Find where code lives by concept — for INITIAL orientation when you don't already know the location. Returns a compact outline of the workspace files whose top-level definitions best match `query`, each line as `<line>  <signature>`. Orientation only: if you already know the exact symbol or string, use grep_search; to find a file by name, use glob_search; to re-read a known file, use read_file. One pass is enough — do NOT call repo_map repeatedly. (Languages: {}.) To list every place a name appears (or pin the real source value past stale docs), call with mode='refs' and name='<exact text>'.",
        map_language_list()
    );
    vec![json!({
        "type": "function",
        "function": {
            "name": "repo_map",
            "description": description,
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What you're looking for, in words or symbol fragments (e.g. 'forge fix loop max rounds default')" },
                    "path":  { "type": "string", "description": "Directory to map (default: the workspace/project root)" },
                    "mode":  { "type": "string", "enum": ["map", "refs"], "description": "map (default): ranked outline of code DEFINITIONS matching a concept. refs: exhaustive deduped list of every place a name appears (definitions, calls, env-var string literals, comments) — source files first, docs last. Use refs to enumerate ALL occurrences of something, or to find the authoritative source value when docs might be stale." },
                    "name":  { "type": "string", "description": "mode=refs only: the exact text to find everywhere, e.g. 'CLAUDETTE_FORGE_' (prefix match finds all CLAUDETTE_FORGE_* vars) or 'DEFAULT_MAX_FIX_ROUNDS'. Case-sensitive literal substring." }
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
    sig: String,
}

#[allow(clippy::too_many_lines)]
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

    // mode="refs": exhaustive occurrence scan. Needle = `name` if given,
    // else fall back to `query` (forgiving — a brain that forgets `name`
    // still works). Default mode is "map" (everything below), so existing
    // behaviour is byte-for-byte unchanged.
    let mode = v.get("mode").and_then(Value::as_str).unwrap_or("map");
    if mode == "refs" {
        let needle = v
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(query);
        return run_repo_refs(&root, needle);
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
            for (_, re) in &patterns {
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
                                sig: line.trim().chars().take(MAX_SIG_CHARS).collect(),
                            },
                        ));
                        break; // one match per line
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

    // Compact, digestible outline instead of nested JSON. The per-symbol JSON
    // keys (`line`/`kind`/`name`/`sig`) — with kind/name duplicating what the
    // sig line already shows — were the bulk of a multi-thousand-token blob the
    // local backend reprocesses on every loop iteration. The outline keeps the
    // same information (file, score, and each matching definition's line +
    // source signature) at a fraction of the size and is easier to parse.
    let mut out = format!(
        "repo_map  \"{query}\"  -  {result_files} of {files_scanned} files matched{}\n",
        if truncated {
            "  (scan hit the file cap; narrow `path` or use grep_search)"
        } else {
            ""
        }
    );
    if result_files == 0 {
        out.push_str(
            "\nNothing matched. Use grep_search for an exact symbol or string, \
             glob_search for a filename, or rephrase the query.\n",
        );
        return Ok(out);
    }
    for (file, score, syms) in scored.into_iter().take(MAX_RESULT_FILES) {
        let _ = writeln!(out, "\n{file}  ({score})");
        for s in &syms {
            let _ = writeln!(out, "  {:>5}  {}", s.line, s.sig);
        }
    }
    out.push_str(
        "\nnext: read_file a cited line for context; grep_search for a known \
         symbol or string; glob_search for a file by name. One orientation pass \
         is enough -- no need to re-run repo_map.\n",
    );
    Ok(out)
}

struct RefHit {
    file: String,
    line: usize,
    sig: String,
}

/// `mode="refs"` — exhaustive, deduped, source-first occurrence scan.
///
/// Unlike map mode (which extracts only top-level *definitions* in 4
/// languages), refs scans the literal text of EVERY readable file
/// (gitignore-aware, build dirs skipped) for `needle` as a case-sensitive
/// substring. This is the only mechanism that surfaces things that are not
/// definitions — env-var string literals (`std::env::var("CLAUDETTE_FORGE_…")`),
/// call sites, doc/comment mentions — which is exactly the enumerate /
/// deep-locate gap map mode can't close.
///
/// Two payloads do the heavy lifting for a weak brain:
/// - `distinct_names`: the deduped set of full identifiers containing the
///   needle, computed over ALL hits before any cap — the brain copies it
///   verbatim as the enumeration answer, no per-line counting.
/// - `source_hits` vs `doc_hits`: source files first, docs quarantined —
///   so the authoritative `const X = 3;` outranks a stale doc saying `2`.
fn run_repo_refs(root: &Path, needle: &str) -> Result<String, String> {
    if needle.trim().is_empty() {
        return Err("repo_map(refs): empty name/query".to_string());
    }

    // Identifier tokens containing the needle → the deduped enumeration
    // answer. BTreeSet keeps it sorted + unique for free.
    let ident_re = Regex::new(r"[A-Za-z_][A-Za-z0-9_]*").expect("static ident regex");
    let mut distinct: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    let mut source_hits: Vec<RefHit> = Vec::new();
    let mut doc_hits: Vec<RefHit> = Vec::new();
    let mut total_hits = 0usize;
    let mut files_scanned = 0usize;
    let mut truncated = false;

    let walker = ignore::WalkBuilder::new(root)
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
        let Ok(meta) = entry.metadata() else { continue };
        if meta.len() > MAX_FILE_BYTES as u64 {
            continue;
        }
        // Language-agnostic: any file that reads as UTF-8 text is scanned
        // (binaries fail read_to_string and are skipped). Docs/config are
        // in scope precisely because they carry the conflicting values.
        let Ok(content) = std::fs::read_to_string(p) else {
            continue;
        };
        files_scanned += 1;
        let source = is_source_file(p);

        for (lineno, line) in content.lines().enumerate() {
            if !line.contains(needle) {
                continue;
            }
            // Enumeration answer: every full identifier on this line that
            // contains the needle. Computed for ALL hits (before the cap).
            for m in ident_re.find_iter(line) {
                if m.as_str().contains(needle) {
                    if distinct.len() < MAX_DISTINCT_NAMES {
                        distinct.insert(m.as_str().to_string());
                    } else {
                        // Enumeration answer is now partial — say so, don't
                        // let the brain treat it as exhaustive.
                        truncated = true;
                    }
                }
            }
            total_hits += 1;
            // PER-PARTITION caps, NOT a joint budget: the `ignore` walker
            // visits dirs alphabetically (docs/ before src/), so a joint cap
            // would let 120+ doc lines exhaust the budget before the first
            // source file is read — starving source_hits and inverting the
            // whole source-first (I3) guarantee. Each partition gets its own
            // MAX_REF_HITS so the authoritative source value always survives.
            let hits = if source {
                &mut source_hits
            } else {
                &mut doc_hits
            };
            if hits.len() < MAX_REF_HITS {
                hits.push(RefHit {
                    file: p.display().to_string(),
                    line: lineno + 1,
                    sig: line.trim().chars().take(MAX_SIG_CHARS).collect(),
                });
            } else {
                truncated = true;
            }
        }
    }

    // Stable, useful order: by file then line within each partition.
    let by_file_line = |a: &RefHit, b: &RefHit| a.file.cmp(&b.file).then(a.line.cmp(&b.line));
    source_hits.sort_by(by_file_line);
    doc_hits.sort_by(by_file_line);

    let to_json = |hits: &[RefHit]| -> Vec<Value> {
        hits.iter()
            .map(|h| json!({ "file": h.file, "line": h.line, "sig": h.sig }))
            .collect()
    };
    let distinct_names: Vec<String> = distinct.into_iter().collect();

    Ok(json!({
        "mode": "refs",
        "needle": needle,
        "root": root.display().to_string(),
        "files_scanned": files_scanned,
        "distinct_names": distinct_names,
        "distinct_count": distinct_names.len(),
        "source_hits": to_json(&source_hits),
        "doc_hits": to_json(&doc_hits),
        "total_hits": total_hits,
        "truncated": truncated,
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
/// One definition pattern: `(kind label, regex source)`. The regex captures the
/// symbol NAME in group 1.
type DefPattern = (&'static str, &'static str);
/// One language for `mode=map`: `(display name, file extensions, patterns)`.
type LangSpec = (&'static str, &'static [&'static str], &'static [DefPattern]);

/// Per-language definition patterns for `mode=map`, as a flat data table.
///
/// ─────────── TO ADD A LANGUAGE: append ONE entry at the end. ───────────
/// `patterns_for` looks it up by extension and the schema's "(Languages: …)"
/// list is generated from the display names — so a new language is a single
/// edit here (plus a CHANGELOG bullet and a test mirroring the others). Each
/// regex captures the symbol name in group 1; order within a language matters
/// (first match per line wins in the scan loop).
const LANG_PATTERNS: &[LangSpec] = &[
    (
        "Rust",
        &["rs"],
        &[
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
    ),
    (
        "Python",
        &["py"],
        &[
            ("def", r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)"),
            ("class", r"^\s*class\s+([A-Za-z_]\w*)"),
        ],
    ),
    (
        "JS/TS",
        &["js", "mjs", "cjs", "jsx", "ts", "tsx"],
        &[
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
    ),
    (
        "Go",
        &["go"],
        &[
            ("func", r"^\s*func\s+(?:\([^)]*\)\s*)?([A-Za-z_]\w*)"),
            ("type", r"^\s*type\s+([A-Za-z_]\w*)"),
            ("const", r"^\s*(?:const|var)\s+([A-Za-z_]\w*)"),
        ],
    ),
    (
        "Ruby",
        &["rb"],
        &[
            ("def", r"^\s*def\s+(?:self\.)?([A-Za-z_]\w*[!?=]?)"),
            ("class", r"^\s*class\s+([A-Za-z_][\w:]*)"),
            ("module", r"^\s*module\s+([A-Za-z_][\w:]*)"),
        ],
    ),
    (
        "C#",
        &["cs"],
        &[
            (
                "class",
                r"^\s*(?:(?:public|private|protected|internal|static|sealed|abstract|partial)\s+)*class\s+([A-Za-z_]\w*)",
            ),
            (
                "interface",
                r"^\s*(?:(?:public|private|protected|internal|partial)\s+)*interface\s+([A-Za-z_]\w*)",
            ),
            (
                "enum",
                r"^\s*(?:(?:public|private|protected|internal)\s+)*enum\s+([A-Za-z_]\w*)",
            ),
            (
                "struct",
                r"^\s*(?:(?:public|private|protected|internal|readonly|partial)\s+)*struct\s+([A-Za-z_]\w*)",
            ),
            (
                "method",
                r"^\s*(?:(?:public|private|protected|internal|static|async|virtual|override|sealed|abstract|extern|new|partial|unsafe)\s+)+[\w<>\[\],.?]+\s+([A-Za-z_]\w*)\s*\(",
            ),
        ],
    ),
    (
        "Java",
        &["java"],
        &[
            (
                "class",
                r"^\s*(?:(?:public|private|protected|static|final|abstract|sealed|strictfp)\s+)*class\s+([A-Za-z_]\w*)",
            ),
            (
                "interface",
                r"^\s*(?:(?:public|private|protected|static|abstract|sealed|strictfp)\s+)*interface\s+([A-Za-z_]\w*)",
            ),
            (
                "enum",
                r"^\s*(?:(?:public|private|protected|static)\s+)*enum\s+([A-Za-z_]\w*)",
            ),
            (
                "method",
                r"^\s*(?:(?:public|private|protected|static|final|abstract|synchronized|native|default|strictfp)\s+)+[\w<>\[\],.?]+\s+([A-Za-z_]\w*)\s*\(",
            ),
        ],
    ),
    (
        "C/C++",
        &["c", "cc", "cpp", "cxx", "h", "hpp", "hh"],
        &[
            ("namespace", r"^\s*namespace\s+([A-Za-z_]\w*)"),
            (
                "class",
                r"^\s*(?:template\s*<[^>]*>\s*)?class\s+([A-Za-z_]\w*)",
            ),
            ("struct", r"^\s*struct\s+([A-Za-z_]\w*)"),
            // enum, enum class, enum struct
            ("enum", r"^\s*enum\s+(?:class\s+|struct\s+)?([A-Za-z_]\w*)"),
            // typedef <stuff> Name;  — captures the last identifier before ';'.
            // Best-effort: function-pointer typedefs (`typedef int (*Cb)(int);`)
            // intentionally don't match (no `Name;` tail).
            ("typedef", r"^\s*typedef\s+.+\b([A-Za-z_]\w*)\s*;"),
            // function/method definition or prototype: <return-type> <name>(.
            // Requires a return type AND a name before '(', so `if (` / `while (`
            // / `for (` / `switch (` and bare calls `foo(` are NOT matched. The
            // `[\s\*&]+` lets the return type carry pointers/refs (`Widget* f(`,
            // `const Foo& g(`). No-lookaround limitation: a `return name(...)`
            // statement line is admitted (accepted — bias to recall in an
            // outline). Constructors have no return type, so they're skipped here
            // and counted once by the `class`/`struct` arm.
            ("fn", r"^\s*(?:[A-Za-z_][\w:]*[\s\*&]+)+([A-Za-z_]\w*)\s*\("),
        ],
    ),
    (
        "PHP",
        &["php"],
        &[
            ("namespace", r"^\s*namespace\s+([A-Za-z_]\w*)"),
            (
                "class",
                r"^\s*(?:(?:abstract|final|readonly)\s+)*class\s+([A-Za-z_]\w*)",
            ),
            ("interface", r"^\s*interface\s+([A-Za-z_]\w*)"),
            ("trait", r"^\s*trait\s+([A-Za-z_]\w*)"),
            ("enum", r"^\s*enum\s+([A-Za-z_]\w*)"),
            // method/function: visibility/static modifiers optional, `function`
            // keyword mandatory (so `if (`/`while (` and `$f = function() {}`
            // anonymous closures are NOT captured — a closure has no name after
            // `function`). `&?` allows return-by-reference (`function &ref()`).
            (
                "function",
                r"^\s*(?:(?:public|private|protected|static|final|abstract)\s+)*function\s+&?\s*([A-Za-z_]\w*)",
            ),
        ],
    ),
    (
        "Kotlin",
        &["kt", "kts"],
        &[
            // Order matters (first match per line wins): class / interface /
            // object precede `fun` so `fun interface Foo` (a functional
            // interface) is captured as an interface, not a function "interface".
            (
                "class",
                r"^\s*(?:(?:public|private|protected|internal|open|final|abstract|sealed|data|inner|enum|annotation|value|expect|actual)\s+)*class\s+([A-Za-z_]\w*)",
            ),
            (
                "interface",
                r"^\s*(?:(?:public|private|protected|internal|sealed|fun|expect|actual)\s+)*interface\s+([A-Za-z_]\w*)",
            ),
            (
                "object",
                r"^\s*(?:(?:public|private|protected|internal|expect|actual)\s+)*(?:companion\s+)?object\s+([A-Za-z_]\w*)",
            ),
            // Functions last. Skips an optional generic list (`fun <T> …`) and a
            // simple extension receiver (`fun String.foo` → captures `foo`).
            (
                "fun",
                r"^\s*(?:(?:public|private|protected|internal|open|final|abstract|override|suspend|inline|operator|infix|tailrec|external|expect|actual)\s+)*fun\s+(?:<[^>]*>\s*)?(?:[A-Za-z_][\w.]*\.)?([A-Za-z_]\w*)",
            ),
        ],
    ),
];

/// Comma-joined display names of the map languages, for the schema description.
fn map_language_list() -> String {
    LANG_PATTERNS
        .iter()
        .map(|(name, _, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

fn patterns_for(path: &Path) -> Option<Vec<(&'static str, Regex)>> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let (_, _, pats) = LANG_PATTERNS
        .iter()
        .find(|(_, exts, _)| exts.contains(&ext.as_str()))?;
    Some(
        pats.iter()
            .filter_map(|&(kind, p)| Regex::new(p).ok().map(|re| (kind, re)))
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
    fn ruby_patterns_capture_def_class_and_module() {
        let pats = patterns_for(Path::new("x.rb")).unwrap();
        // plain def
        let line_def = "  def method_name";
        let hit_def = pats
            .iter()
            .find_map(|(k, re)| re.captures(line_def).map(|c| (*k, c[1].to_string())));
        assert_eq!(hit_def, Some(("def", "method_name".to_string())));

        // def self.class_method - must extract class_method, not self
        let line_self = "  def self.class_method";
        let hit_self = pats
            .iter()
            .find_map(|(k, re)| re.captures(line_self).map(|c| (*k, c[1].to_string())));
        assert_eq!(hit_self, Some(("def", "class_method".to_string())));

        // def with ? - must keep the ?
        let line_q = "  def valid?";
        let hit_q = pats
            .iter()
            .find_map(|(k, re)| re.captures(line_q).map(|c| (*k, c[1].to_string())));
        assert_eq!(hit_q, Some(("def", "valid?".to_string())));

        // class with namespace
        let line_class = "  class Foo::Bar";
        let hit_class = pats
            .iter()
            .find_map(|(k, re)| re.captures(line_class).map(|c| (*k, c[1].to_string())));
        assert_eq!(hit_class, Some(("class", "Foo::Bar".to_string())));

        // module
        let line_mod = "  module Something";
        let hit_mod = pats
            .iter()
            .find_map(|(k, re)| re.captures(line_mod).map(|c| (*k, c[1].to_string())));
        assert_eq!(hit_mod, Some(("module", "Something".to_string())));
    }

    #[test]
    fn repo_map_ruby_definitions_extracted_in_map_mode() {
        let _eg = crate::test_env_lock();
        let base = super::super::user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-repomap-test-ruby");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("lib")).unwrap();
        let ruby_content = "module Something\n  class Greeting\n    def initialize(name)\n      @name = name\n    end\n\n    def self.class_method\n      \"class\"\n    end\n\n    def valid?\n      !@name.nil?\n    end\n\n    def say_hello\n      \"Hello, #{@name}\"\n    end\n  end\nend\n";
        std::fs::write(base.join("lib").join("greeting.rb"), ruby_content.trim()).unwrap();

        let input = json!({
            "query": "greeting module class method",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_repo_map(&input).unwrap().replace('\\', "/");

        assert!(
            out.contains("lib/greeting.rb"),
            "greeting.rb should be found:\n{out}"
        );
        // The outline shows each definition's source line (signature), which
        // already carries the kind keyword and the name — so the extraction is
        // proven by the sig appearing, with no separate name/kind fields.
        for sig in [
            "module Something",
            "class Greeting",
            "def initialize",
            "def self.class_method",
            "def valid?",
            "def say_hello",
        ] {
            assert!(out.contains(sig), "expected `{sig}` in the outline:\n{out}");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn kotlin_patterns_capture_class_interface_object_and_fun() {
        let pats = patterns_for(Path::new("x.kt")).unwrap();
        let hit = |line: &str| {
            pats.iter()
                .find_map(|(k, re)| re.captures(line).map(|c| (*k, c[1].to_string())))
        };

        // plain + modified functions
        assert_eq!(hit("fun main() {"), Some(("fun", "main".to_string())));
        assert_eq!(
            hit("    private suspend fun fetch(id: Int): User {"),
            Some(("fun", "fetch".to_string()))
        );
        // generic function — skip <T> and capture the name
        assert_eq!(
            hit("fun <T> singletonList(item: T): List<T> {"),
            Some(("fun", "singletonList".to_string()))
        );
        // extension function — receiver stripped, member name captured
        assert_eq!(
            hit("fun String.isPalindrome(): Boolean {"),
            Some(("fun", "isPalindrome".to_string()))
        );

        // classes with modifiers / kinds
        assert_eq!(
            hit("class Greeting(val name: String) {"),
            Some(("class", "Greeting".to_string()))
        );
        assert_eq!(
            hit("data class Point(val x: Int, val y: Int)"),
            Some(("class", "Point".to_string()))
        );
        assert_eq!(
            hit("enum class Color { RED, GREEN }"),
            Some(("class", "Color".to_string()))
        );
        assert_eq!(
            hit("sealed class Shape"),
            Some(("class", "Shape".to_string()))
        );

        // interface, incl. the `fun interface` disambiguation
        assert_eq!(
            hit("interface Repository {"),
            Some(("interface", "Repository".to_string()))
        );
        assert_eq!(
            hit("fun interface Runnable {"),
            Some(("interface", "Runnable".to_string()))
        );

        // object + companion object
        assert_eq!(
            hit("object Singleton {"),
            Some(("object", "Singleton".to_string()))
        );
        assert_eq!(
            hit("    companion object Factory {"),
            Some(("object", "Factory".to_string()))
        );
    }

    #[test]
    fn repo_map_kotlin_definitions_extracted_in_map_mode() {
        let _eg = crate::test_env_lock();
        let base = super::super::user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-repomap-test-kotlin");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        let kotlin_content = "package demo\n\ninterface Greeter {\n    fun greet(name: String): String\n}\n\nclass Greeting(val prefix: String) : Greeter {\n    override fun greet(name: String): String = \"$prefix, $name\"\n}\n\nobject Config {\n    const val VERSION = \"1.0\"\n}\n\nfun main() {\n    println(Greeting(\"Hello\").greet(\"world\"))\n}\n";
        std::fs::write(base.join("src").join("Greeting.kt"), kotlin_content).unwrap();

        let input = json!({
            "query": "greeter greeting config main",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_repo_map(&input).unwrap().replace('\\', "/");

        assert!(
            out.contains("src/Greeting.kt"),
            "Greeting.kt should be found:\n{out}"
        );
        for sig in [
            "interface Greeter",
            "class Greeting",
            "object Config",
            "fun main",
        ] {
            assert!(out.contains(sig), "expected `{sig}` in the outline:\n{out}");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn csharp_patterns_capture_class_interface_enum_and_method() {
        let pats = patterns_for(Path::new("x.cs")).unwrap();
        let hit = |line: &str| {
            pats.iter()
                .find_map(|(k, re)| re.captures(line).map(|c| (*k, c[1].to_string())))
        };

        assert_eq!(
            hit("    public class Foo"),
            Some(("class", "Foo".to_string()))
        );
        assert_eq!(
            hit("public interface IBar"),
            Some(("interface", "IBar".to_string()))
        );
        assert_eq!(
            hit("    internal enum Color"),
            Some(("enum", "Color".to_string()))
        );
        assert_eq!(
            hit("public struct Point"),
            Some(("struct", "Point".to_string()))
        );
        // method = modifier(s) + return type + name + '('
        assert_eq!(
            hit("    public void Baz()"),
            Some(("method", "Baz".to_string()))
        );
        assert_eq!(
            hit("    private static int Add(int a, int b)"),
            Some(("method", "Add".to_string()))
        );
        assert_eq!(
            hit("    public async Task<int> GetAsync()"),
            Some(("method", "GetAsync".to_string()))
        );
        // control flow must NOT be captured as a method (no keyword swallow)
        assert_eq!(hit("    if (ready)"), None);
        assert_eq!(hit("    return Add(1, 2);"), None);
    }

    #[test]
    fn java_patterns_capture_class_interface_enum_and_method() {
        let pats = patterns_for(Path::new("x.java")).unwrap();
        let hit = |line: &str| {
            pats.iter()
                .find_map(|(k, re)| re.captures(line).map(|c| (*k, c[1].to_string())))
        };

        assert_eq!(hit("public class Foo"), Some(("class", "Foo".to_string())));
        assert_eq!(
            hit("    public final class Bar"),
            Some(("class", "Bar".to_string()))
        );
        assert_eq!(
            hit("public interface IBar"),
            Some(("interface", "IBar".to_string()))
        );
        // enum allows zero modifiers
        assert_eq!(hit("    enum Color"), Some(("enum", "Color".to_string())));
        // method = modifier(s) + return type + name + '('
        assert_eq!(
            hit("    public void run()"),
            Some(("method", "run".to_string()))
        );
        assert_eq!(
            hit("    public static int add(int a, int b)"),
            Some(("method", "add".to_string()))
        );
        // control flow must NOT be captured as a method (no keyword swallow)
        assert_eq!(hit("    if (ready) {"), None);
        assert_eq!(hit("    while (true) {"), None);
        assert_eq!(hit("    return add(1, 2);"), None);
    }

    #[test]
    fn repo_map_java_definitions_extracted_in_map_mode() {
        let _eg = crate::test_env_lock();
        let base = super::super::user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-repomap-test-java");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        let java_content = "package app;\n\npublic interface Greeter {\n    String greet(String name);\n}\n\npublic class HelloGreeter implements Greeter {\n    private final String prefix;\n\n    public HelloGreeter(String prefix) {\n        this.prefix = prefix;\n    }\n\n    public String greet(String name) {\n        return prefix + name;\n    }\n\n    public static int add(int a, int b) {\n        return a + b;\n    }\n}\n\npublic enum Mood {\n    HAPPY,\n    SAD,\n}\n";
        std::fs::write(base.join("src").join("HelloGreeter.java"), java_content).unwrap();

        let input = json!({
            "query": "greeter class interface method add enum",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_repo_map(&input).unwrap().replace('\\', "/");

        assert!(
            out.contains("src/HelloGreeter.java"),
            "HelloGreeter.java should be found:\n{out}"
        );
        for sig in [
            "public interface Greeter",
            "public class HelloGreeter",
            "public String greet",
            "public static int add",
            "public enum Mood",
        ] {
            assert!(out.contains(sig), "missing `{sig}` in:\n{out}");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn repo_map_csharp_definitions_extracted_in_map_mode() {
        let _eg = crate::test_env_lock();
        let base = super::super::user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-repomap-test-csharp");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        let cs_content = "namespace App\n{\n    public interface IGreeter\n    {\n        string Greet(string name);\n    }\n\n    public class Greeter : IGreeter\n    {\n        private readonly string _prefix;\n\n        public Greeter(string prefix)\n        {\n            _prefix = prefix;\n        }\n\n        public string Greet(string name)\n        {\n            return _prefix + name;\n        }\n\n        public static int Add(int a, int b)\n        {\n            return a + b;\n        }\n    }\n\n    public enum Mood\n    {\n        Happy,\n        Sad,\n    }\n}\n";
        std::fs::write(base.join("src").join("Greeter.cs"), cs_content).unwrap();

        let input = json!({
            "query": "greeter class interface method add enum",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_repo_map(&input).unwrap().replace('\\', "/");

        assert!(
            out.contains("src/Greeter.cs"),
            "Greeter.cs should be found:\n{out}"
        );
        // Each sig appears in the outline (kind keyword + name come from the line).
        for sig in [
            "public interface IGreeter",
            "public class Greeter",
            "public string Greet",
            "public static int Add",
            "public enum Mood",
        ] {
            assert!(out.contains(sig), "missing `{sig}` in:\n{out}");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cpp_patterns_capture_class_struct_namespace_enum_typedef_and_function() {
        let pats = patterns_for(Path::new("x.cpp")).unwrap();
        let hit = |line: &str| {
            pats.iter()
                .find_map(|(k, re)| re.captures(line).map(|c| (*k, c[1].to_string())))
        };

        assert_eq!(hit("namespace baz"), Some(("namespace", "baz".to_string())));
        assert_eq!(
            hit("class Widget : public Base"),
            Some(("class", "Widget".to_string()))
        );
        assert_eq!(hit("struct Bar"), Some(("struct", "Bar".to_string())));
        assert_eq!(hit("enum class Color"), Some(("enum", "Color".to_string())));
        assert_eq!(hit("enum Mood"), Some(("enum", "Mood".to_string())));
        assert_eq!(
            hit("typedef struct Point Point_t;"),
            Some(("typedef", "Point_t".to_string()))
        );
        // functions: with/without modifiers, pointer + qualified return types
        assert_eq!(
            hit("int add(int a, int b)"),
            Some(("fn", "add".to_string()))
        );
        assert_eq!(
            hit("    static void run()"),
            Some(("fn", "run".to_string()))
        );
        assert_eq!(hit("Widget* make(int n)"), Some(("fn", "make".to_string())));
        assert_eq!(
            hit("std::string greet(const std::string& name)"),
            Some(("fn", "greet".to_string()))
        );
        // keyword-swallow guard: control flow is NOT a function (no name before `(`)
        assert_eq!(hit("    if (ready) {"), None);
        assert_eq!(hit("    while (true) {"), None);
        assert_eq!(hit("    for (int i = 0; i < n; i++) {"), None);
        assert_eq!(hit("    switch (state) {"), None);
        // bare call / constructor have no return type → not captured as fn
        assert_eq!(hit("    doThing(x);"), None);
        assert_eq!(hit("    Widget(int n)"), None);
    }

    #[test]
    fn repo_map_cpp_definitions_extracted_in_map_mode() {
        let _eg = crate::test_env_lock();
        let base = super::super::user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-repomap-test-cpp");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        let cpp_content = "namespace geo {\n\nstruct Point {\n    int x;\n    int y;\n};\n\nenum class Shape {\n    Circle,\n    Square,\n};\n\nclass Widget {\npublic:\n    Widget(int n);\n    int area() const;\n};\n\nint add(int a, int b) {\n    return add_impl(a, b);\n}\n\n}\n";
        std::fs::write(base.join("src").join("widget.cpp"), cpp_content).unwrap();

        let input = json!({
            "query": "widget point shape namespace geo add area",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_repo_map(&input).unwrap().replace('\\', "/");

        assert!(
            out.contains("src/widget.cpp"),
            "widget.cpp should be found:\n{out}"
        );
        for sig in [
            "namespace geo",
            "struct Point",
            "enum class Shape",
            "class Widget",
            "int add(int a, int b)",
        ] {
            assert!(out.contains(sig), "missing `{sig}` in:\n{out}");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn php_patterns_capture_class_interface_trait_enum_namespace_and_function() {
        let pats = patterns_for(Path::new("x.php")).unwrap();
        let hit = |line: &str| {
            pats.iter()
                .find_map(|(k, re)| re.captures(line).map(|c| (*k, c[1].to_string())))
        };

        assert_eq!(
            hit("namespace App;"),
            Some(("namespace", "App".to_string()))
        );
        assert_eq!(hit("class Greeter"), Some(("class", "Greeter".to_string())));
        assert_eq!(
            hit("abstract class Base"),
            Some(("class", "Base".to_string()))
        );
        assert_eq!(
            hit("interface Greeter"),
            Some(("interface", "Greeter".to_string()))
        );
        assert_eq!(hit("trait Hello"), Some(("trait", "Hello".to_string())));
        assert_eq!(hit("enum Suit"), Some(("enum", "Suit".to_string())));
        assert_eq!(
            hit("    public function greet($name)"),
            Some(("function", "greet".to_string()))
        );
        assert_eq!(
            hit("    public static function add($a, $b)"),
            Some(("function", "add".to_string()))
        );
        assert_eq!(
            hit("function greet()"),
            Some(("function", "greet".to_string()))
        );
        // control flow and anonymous closures are NOT functions (no name after `function`)
        assert_eq!(hit("    if ($ready) {"), None);
        assert_eq!(hit("    while (true) {"), None);
        assert_eq!(hit("    $f = function() {"), None);
        assert_eq!(hit("    doThing($x);"), None);
    }

    #[test]
    fn repo_map_php_definitions_extracted_in_map_mode() {
        let _eg = crate::test_env_lock();
        let base = super::super::user_home()
            .join(".claudette")
            .join("files")
            .join("claudette-repomap-test-php");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        let php_content = "<?php\n\nnamespace App;\n\ninterface Greeter {\n    public function greet(string $name): string;\n}\n\nclass HelloGreeter implements Greeter {\n    private string $prefix;\n\n    public function __construct(string $prefix) {\n        $this->prefix = $prefix;\n    }\n\n    public function greet(string $name): string {\n        return $this->prefix . $name;\n    }\n\n    public static function add(int $a, int $b): int {\n        return $a + $b;\n    }\n}\n\nenum Mood {\n    case Happy;\n    case Sad;\n}\n";
        std::fs::write(base.join("src").join("Greeter.php"), php_content).unwrap();

        let input = json!({
            "query": "greeter class interface function greet add enum namespace",
            "path": base.to_str().unwrap()
        })
        .to_string();
        let out = run_repo_map(&input).unwrap().replace('\\', "/");

        assert!(
            out.contains("src/Greeter.php"),
            "Greeter.php should be found:\n{out}"
        );
        for sig in [
            "namespace App",
            "interface Greeter",
            "class HelloGreeter",
            "public function greet",
            "public static function add",
            "enum Mood",
        ] {
            assert!(out.contains(sig), "missing `{sig}` in:\n{out}");
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn schema_language_list_is_derived_from_the_table() {
        // The whole point of the LANG_PATTERNS refactor: adding a language to the
        // table surfaces in the schema "(Languages: …)" list automatically.
        let schemas = schemas();
        let desc = schemas[0]["function"]["description"].as_str().unwrap();
        assert!(
            desc.contains(&format!("(Languages: {}.)", map_language_list())),
            "schema description must embed the generated language list: {desc}"
        );
        // the table actually carries the languages we claim to support
        let list = map_language_list();
        for lang in ["Rust", "Python", "C#", "Java", "C/C++", "PHP"] {
            assert!(
                list.contains(lang),
                "table missing language `{lang}`: {list}"
            );
        }
    }

    #[test]
    fn repo_map_ranks_the_matching_file_first_with_value_in_sig() {
        let _eg = crate::test_env_lock(); // home-resolving: serialize vs temp-home swaps
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
        let out = run_repo_map(&input).unwrap().replace('\\', "/");

        // run.rs is the matching file; the const's sig carries the value (= 3).
        assert!(
            out.contains("src/run.rs"),
            "run.rs should be listed:\n{out}"
        );
        assert!(out.contains("= 3"), "expected the value in a sig:\n{out}");
        // notes.rs shares no query token → excluded from the outline.
        assert!(
            !out.contains("notes.rs"),
            "non-matching file must be excluded:\n{out}"
        );
        // Map output, not refs: no refs-only section markers.
        assert!(!out.contains("source_hits"), "map mode is not refs:\n{out}");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Build a throwaway fixture tree under a unique dir, run a closure with
    /// its path, then clean up. Holds the env lock (home-resolving).
    fn with_refs_fixture<F: FnOnce(&str)>(tag: &str, files: &[(&str, &str)], f: F) {
        let _eg = crate::test_env_lock();
        let base = super::super::user_home()
            .join(".claudette")
            .join("files")
            .join(format!("claudette-refs-test-{tag}"));
        let _ = std::fs::remove_dir_all(&base);
        for (rel, content) in files {
            let p = base.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
        }
        f(base.to_str().unwrap());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn repo_refs_enumerates_all_distinct_prefix_matches() {
        // I1 proxy: the 6 CLAUDETTE_FORGE_* vars exist ONLY as env-read
        // string literals / comments / error strings — ZERO definitions —
        // so map mode finds none. refs must enumerate all 6, deduped across
        // duplicate lines and across files.
        let run_rs = r#"
            // CLAUDETTE_FORGE_ABORT_WINDOW_SECS controls the abort window
            let _ = std::env::var("CLAUDETTE_FORGE_ABORT_WINDOW_SECS");
            if env_flag_enabled("CLAUDETTE_FORGE_ALLOW_DIRTY") {}
            if env_flag_enabled("CLAUDETTE_FORGE_ALLOW_DIRTY") {} // duplicate line
            let _ = std::env::var("CLAUDETTE_FORGE_AUTO_APPROVE");
            let _ = std::env::var("CLAUDETTE_FORGE_SUBMIT_ON_FAIL");
        "#;
        let sec_rs = r#"
            if env_flag_enabled("CLAUDETTE_FORGE_SECURITY_OVERRIDE") {}
            // also CLAUDETTE_FORGE_SECURITY_REVIEW in this comment
        "#;
        with_refs_fixture(
            "i1",
            &[("src/run.rs", run_rs), ("src/security_review.rs", sec_rs)],
            |path| {
                let input = json!({
                    "query": "forge env vars",
                    "mode": "refs",
                    "name": "CLAUDETTE_FORGE_",
                    "path": path
                })
                .to_string();
                let out = run_repo_map(&input).unwrap();
                let v: Value = serde_json::from_str(&out).unwrap();
                assert_eq!(v["mode"], "refs");
                let names: Vec<&str> = v["distinct_names"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|n| n.as_str().unwrap())
                    .collect();
                let expected = [
                    "CLAUDETTE_FORGE_ABORT_WINDOW_SECS",
                    "CLAUDETTE_FORGE_ALLOW_DIRTY",
                    "CLAUDETTE_FORGE_AUTO_APPROVE",
                    "CLAUDETTE_FORGE_SECURITY_OVERRIDE",
                    "CLAUDETTE_FORGE_SECURITY_REVIEW",
                    "CLAUDETTE_FORGE_SUBMIT_ON_FAIL",
                ];
                for e in expected {
                    assert!(names.contains(&e), "missing {e} in {names:?}");
                }
                assert_eq!(v["distinct_count"], 6, "deduped to exactly 6: {names:?}");
            },
        );
    }

    #[test]
    fn repo_refs_puts_source_before_docs_with_value_in_sig() {
        // I3 proxy: the real const is in run.rs (= 3); stale docs say 2. The
        // source hit must carry the value, and every doc_hit must be a .md.
        with_refs_fixture(
            "i3",
            &[
                ("src/run.rs", "const DEFAULT_MAX_FIX_ROUNDS: u32 = 3;\n"),
                ("docs/forge.md", "The DEFAULT_MAX_FIX_ROUNDS is 2 rounds.\n"),
                (
                    "docs/configuration.md",
                    "Set DEFAULT_MAX_FIX_ROUNDS (default 2).\n",
                ),
            ],
            |path| {
                let input = json!({
                    "query": "max fix rounds",
                    "mode": "refs",
                    "name": "DEFAULT_MAX_FIX_ROUNDS",
                    "path": path
                })
                .to_string();
                let out = run_repo_map(&input).unwrap();
                let v: Value = serde_json::from_str(&out).unwrap();
                let src = v["source_hits"].as_array().unwrap();
                assert!(!src.is_empty(), "source hit expected");
                assert!(
                    src[0]["file"]
                        .as_str()
                        .unwrap()
                        .replace('\\', "/")
                        .ends_with("/src/run.rs"),
                    "source-first: {out}"
                );
                assert!(
                    src[0]["sig"].as_str().unwrap().contains("= 3"),
                    "source hit must carry the real value: {out}"
                );
                for d in v["doc_hits"].as_array().unwrap() {
                    // Fixture filenames are literal lowercase `.md`, so a
                    // case-sensitive check is exactly what we want here.
                    #[allow(clippy::case_sensitive_file_extension_comparisons)]
                    let is_md = d["file"].as_str().unwrap().ends_with(".md");
                    assert!(is_md, "doc partition must hold only docs: {out}");
                }
            },
        );
    }

    #[test]
    fn repo_refs_forgiving_args_falls_back_to_query() {
        // Weak-brain forgiving args: mode=refs with NO `name` uses `query`.
        with_refs_fixture(
            "fallback",
            &[(
                "src/run.rs",
                "std::env::var(\"CLAUDETTE_FORGE_AUTO_APPROVE\");\n",
            )],
            |path| {
                let input = json!({
                    "query": "CLAUDETTE_FORGE_",
                    "mode": "refs",
                    "path": path
                })
                .to_string();
                let out = run_repo_map(&input).unwrap();
                let v: Value = serde_json::from_str(&out).unwrap();
                assert_eq!(
                    v["distinct_count"], 1,
                    "fell back to query as needle: {out}"
                );
            },
        );
    }

    #[test]
    fn repo_refs_source_hits_survive_a_flood_of_earlier_doc_hits() {
        // I3 guarantee under load: the walker visits docs/ before src/
        // alphabetically. With a JOINT hit cap, 130 doc matches would
        // exhaust the budget and the lone authoritative source hit would be
        // dropped — silently inverting source-first. Per-partition caps must
        // keep the source hit. (MAX_REF_HITS=120; 130 doc lines > that.)
        let many_doc_lines = "MARKER_NEEDLE mentioned in the docs\n".repeat(130);
        with_refs_fixture(
            "flood",
            &[
                ("docs/big.md", &many_doc_lines),
                ("src/run.rs", "const MARKER_NEEDLE: u32 = 3;\n"),
            ],
            |path| {
                let input = json!({
                    "query": "x", "mode": "refs", "name": "MARKER_NEEDLE", "path": path
                })
                .to_string();
                let out = run_repo_map(&input).unwrap();
                let v: Value = serde_json::from_str(&out).unwrap();
                let src = v["source_hits"].as_array().unwrap();
                assert!(
                    !src.is_empty(),
                    "source hit must NOT be starved by 130 earlier doc hits: {}",
                    // truncated should be flagged; doc_hits capped at 120.
                    serde_json::to_string(&json!({
                        "source_len": src.len(),
                        "doc_len": v["doc_hits"].as_array().unwrap().len(),
                        "truncated": v["truncated"].clone(),
                    }))
                    .unwrap()
                );
                assert!(
                    src[0]["sig"].as_str().unwrap().contains("= 3"),
                    "the surviving source hit must be the real value"
                );
                assert_eq!(
                    v["truncated"], true,
                    "doc flood past the cap must flag truncated"
                );
            },
        );
    }

    #[test]
    fn repo_refs_finds_string_literals_not_only_definitions() {
        // Guard against regressing into definition-only scanning (the
        // rejected Design-1 failure mode): a needle with ZERO definitions
        // but many string-literal occurrences must still produce source hits.
        with_refs_fixture(
            "literals",
            &[(
                "src/run.rs",
                "let a = \"CLAUDETTE_FORGE_X\";\nlet b = \"CLAUDETTE_FORGE_Y\";\n",
            )],
            |path| {
                let input = json!({
                    "query": "x", "mode": "refs", "name": "CLAUDETTE_FORGE_", "path": path
                })
                .to_string();
                let out = run_repo_map(&input).unwrap();
                let v: Value = serde_json::from_str(&out).unwrap();
                assert!(
                    !v["source_hits"].as_array().unwrap().is_empty(),
                    "string-literal occurrences must be found: {out}"
                );
            },
        );
    }
}
