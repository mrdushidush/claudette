//! Doc-vs-binary truth guard (roast 2026-06-30, H4 + Theme D).
//!
//! The 2026-06-30 roast found a whole `markets`/TradingView tool group
//! advertised across six live docs that had already been amputated from the
//! binary (`tool_groups.rs` asserts `parse("markets") == None`), plus a
//! tool-group count stated three different ways (README/architecture said 22,
//! a `prompt.rs` comment said 17, the code says 21). These two tests keep the
//! user-facing docs honest so the rot can't return silently:
//!
//! 1. No live doc names an amputated tool group (`markets` / `tradingview` /
//!    `vestige` / `tv_get_quote`).
//! 2. The tool-group count the docs cite matches `ToolGroup::all().len()`.
//!
//! Historical records are exempt: `CHANGELOG.md` and everything under
//! `docs/archive/` legitimately reference the removed feature, so the scan
//! covers only the live surface (`README.md`, `PRIVACY.md`, top-level
//! `docs/*.md`).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use claudette::tool_groups::ToolGroup;

/// Repo root, derived from the crate manifest dir (`crates/claudette` → `../..`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// The live, user-facing docs: `README.md`, `PRIVACY.md`, and the top-level
/// `docs/*.md` files. `docs/archive/` is a subdirectory, so the non-recursive
/// `read_dir` walk skips it; `CHANGELOG.md` is excluded by not being listed.
fn live_doc_files() -> Vec<PathBuf> {
    let root = repo_root();
    let mut files = vec![root.join("README.md"), root.join("PRIVACY.md")];
    let docs = root.join("docs");
    for entry in fs::read_dir(&docs).expect("docs/ directory should exist") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|e| e.to_str()) == Some("md") {
            files.push(path);
        }
    }
    files
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Lowercased word set, splitting on anything that isn't `[A-Za-z0-9_]`.
/// Underscores stay (so `tv_get_quote` survives as one token), which also
/// means whole-word matching: `marketplace` / `marketing` tokenize to
/// themselves and never collide with the forbidden `markets`.
fn word_set(content: &str) -> BTreeSet<String> {
    content
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

#[test]
fn live_docs_name_no_amputated_tool_groups() {
    // Each of these was a tool/group removed from the binary; `tool_groups.rs`
    // proves the amputation in its own unit tests. A doc that still names one
    // is advertising a feature the shipped binary cannot deliver.
    const FORBIDDEN: &[&str] = &["markets", "tradingview", "vestige", "tv_get_quote"];

    let mut offenders: Vec<String> = Vec::new();
    for path in live_doc_files() {
        let words = word_set(&read(&path));
        for &bad in FORBIDDEN {
            if words.contains(bad) {
                offenders.push(format!("{} references `{bad}`", path.display()));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "live docs reference amputated tool groups (they no longer exist in the \
         binary — see tool_groups.rs):\n  {}",
        offenders.join("\n  ")
    );
}

#[test]
fn live_docs_cite_the_real_tool_group_count() {
    let n = ToolGroup::all().len();

    // README and comparison.md phrase it as "N opt-in [tool] groups".
    for rel in ["README.md", "docs/comparison.md"] {
        let body = read(&repo_root().join(rel));
        assert!(
            body.contains(&format!("{n} opt-in")),
            "{rel} must cite the live tool-group count as `{n} opt-in …` \
             (ToolGroup::all().len() == {n}); update it when groups change"
        );
    }

    // architecture.md phrases it as "N groups, ~80 tools".
    let arch = read(&repo_root().join("docs/architecture.md"));
    assert!(
        arch.contains(&format!("{n} groups")),
        "docs/architecture.md must cite the live tool-group count as `{n} groups` \
         (ToolGroup::all().len() == {n}); update it when groups change"
    );
}
