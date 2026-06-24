//! Model-free verify slice of the eval battery (Wave F.2).
//!
//! The full eval battery at `runs/eval-2026-05-29/battery/` is model-driven and
//! therefore flaky in CI (LM-Studio eviction, generation nondeterminism), so it
//! has never run as a gate. This target carves out the *deterministic* core: it
//! takes the battery's real fixtures and applies the **golden** fix for each
//! task through claudette's actual edit-tool dispatch path
//! ([`dispatch_tool`](claudette::tools::dispatch_tool)), with NO model in the
//! loop, then asserts the file reached the solved state.
//!
//! Why it matters: Wave B collapses the overlapping edit tools (`edit_file` /
//! `apply_diff` / `apply_patch`). This test pins their observable behaviour on
//! real files so a regression in that refactor fails CI on every PR instead of
//! hiding behind a model-eviction flake. It runs under the existing
//! `cargo test --tests` job — no workflow change.
//!
//! Determinism: codet post-edit validation is disabled
//! (`CLAUDETTE_VALIDATE_CODE=false`) so no language toolchain is shelled out,
//! and assertions are pure file-content checks (the equivalent of the battery's
//! source-level verifiers — e.g. `verify/E1.sh` already greps the source rather
//! than compiling). Fixtures are copied into a throwaway workspace under
//! `CARGO_TARGET_TMPDIR`, and `CLAUDETTE_WORKSPACE` points there so the edit
//! tools' path sandbox permits the writes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use claudette::tools::dispatch_tool;
use serde_json::{json, Value};

/// Locate the committed battery directory. Overridable with
/// `CLAUDETTE_BATTERY_DIR` (mirrors the harness's other path overrides);
/// defaults to `<repo>/runs/eval-2026-05-29/battery` relative to this crate.
fn battery_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDETTE_BATTERY_DIR") {
        return PathBuf::from(dir);
    }
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("runs")
        .join("eval-2026-05-29")
        .join("battery")
}

/// One-time setup: a throwaway workspace root the edit-tool path sandbox
/// accepts, with codet validation disabled for determinism. Returns the root
/// every task copies its fixture under. `OnceLock` makes the two `set_var`
/// calls happen exactly once and complete before any test reads the
/// environment, avoiding a set/read race across this binary's parallel test
/// threads (every test calls this before it dispatches a tool).
fn workspace_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = Path::new(env!("CARGO_TARGET_TMPDIR")).join("battery_verify");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create battery_verify workspace root");
        std::env::set_var("CLAUDETTE_WORKSPACE", &root);
        std::env::set_var("CLAUDETTE_VALIDATE_CODE", "false");
        root
    })
}

/// Copy battery fixture `id` into a fresh, uniquely-named per-call subdir of the
/// workspace and return that working directory. The unique suffix lets the same
/// fixture be prepared by several parallel tests without clobbering.
fn prepare(id: &str) -> PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let src = battery_dir().join("fixtures").join(id);
    assert!(
        src.is_dir(),
        "battery fixture {id} not found at {} — is the battery checked out?",
        src.display()
    );
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let dst = workspace_root().join(format!("{id}-{n}"));
    copy_dir(&src, &dst);
    dst
}

fn copy_dir(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("create dst dir");
    for entry in std::fs::read_dir(src).expect("read_dir fixture") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let target = dst.join(entry.file_name());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy fixture file");
        }
    }
}

fn read(workdir: &Path, rel: &str) -> String {
    std::fs::read_to_string(workdir.join(rel))
        .unwrap_or_else(|e| panic!("read {rel} in {}: {e}", workdir.display()))
}

/// Dispatch `tool` with `input` and require a success result (`"ok":true`).
fn apply_ok(tool: &str, input: &Value) {
    let out = dispatch_tool(tool, &input.to_string())
        .unwrap_or_else(|e| panic!("{tool} should succeed but errored: {e}"));
    assert!(
        out.contains("\"ok\":true"),
        "{tool} did not report ok:true — {out}"
    );
}

/// JSON-friendly absolute path to `rel` inside `workdir`.
fn abs(workdir: &Path, rel: &str) -> String {
    workdir.join(rel).to_string_lossy().into_owned()
}

#[test]
fn edit_file_applies_golden_bugfixes() {
    // A1 — Rust: integer division before the f64 cast.
    let wd = prepare("A1");
    apply_ok(
        "edit_file",
        &json!({
            "path": abs(&wd, "src/lib.rs"),
            "old_text": "(total / xs.len() as i64) as f64",
            "new_text": "(total as f64 / xs.len() as f64)",
        }),
    );
    let c = read(&wd, "src/lib.rs");
    assert!(
        !c.contains("xs.len() as i64"),
        "A1: buggy cast remains:\n{c}"
    );
    assert!(
        c.contains("total as f64 / xs.len() as f64"),
        "A1: fix not applied:\n{c}"
    );

    // B1 — Python: off-by-one denominator in mean().
    let wd = prepare("B1");
    apply_ok(
        "edit_file",
        &json!({
            "path": abs(&wd, "stats.py"),
            "old_text": "sum(xs) / (len(xs) - 1)",
            "new_text": "sum(xs) / len(xs)",
        }),
    );
    let c = read(&wd, "stats.py");
    assert!(!c.contains("(len(xs) - 1)"), "B1: bug remains:\n{c}");
    assert!(c.contains("sum(xs) / len(xs)"), "B1: fix missing:\n{c}");

    // C1 — JS: cart total ignores quantity.
    let wd = prepare("C1");
    apply_ok(
        "edit_file",
        &json!({
            "path": abs(&wd, "cart.js"),
            "old_text": "total += item.price;",
            "new_text": "total += item.price * item.qty;",
        }),
    );
    let c = read(&wd, "cart.js");
    assert!(c.contains("item.price * item.qty"), "C1: fix missing:\n{c}");

    // D1 — TS: email regex accepts addresses without an "@".
    let wd = prepare("D1");
    apply_ok(
        "edit_file",
        &json!({
            "path": abs(&wd, "validate.ts"),
            "old_text": "/.+\\..+/",
            "new_text": "/.+@.+\\..+/",
        }),
    );
    let c = read(&wd, "validate.ts");
    assert!(c.contains("/.+@.+\\..+/"), "D1: fix missing:\n{c}");
}

#[test]
fn apply_diff_applies_golden_fix() {
    // E1 — Go: Max returns the smaller value; flip the returns.
    let wd = prepare("E1");
    apply_ok(
        "apply_diff",
        &json!({
            "path": abs(&wd, "calc.go"),
            // Include the leading tab the file actually has on the `if` line:
            // apply_diff's fuzzy matcher re-indents the `after` block by the
            // offset between the `before` block's indent and the match's, so a
            // mismatched first-line indent would corrupt the result.
            "before": "\tif a < b {\n\t\treturn a\n\t}\n\treturn b",
            "after": "\tif a < b {\n\t\treturn b\n\t}\n\treturn a",
        }),
    );
    let c = read(&wd, "calc.go");
    assert!(
        !c.contains("if a < b {\n\t\treturn a"),
        "E1: buggy branch remains:\n{c}"
    );
    assert!(
        c.contains("if a < b {\n\t\treturn b"),
        "E1: fix not applied:\n{c}"
    );
}

#[test]
fn apply_patch_applies_golden_fix() {
    // F1 — shell: greeting is missing the trailing "!". A precise single-hunk
    // unified diff. The header path is absolute (forward-slashed) with the
    // git-style `b/` prefix that apply_patch strips, so it resolves regardless
    // of the process cwd.
    let wd = prepare("F1");
    let path = abs(&wd, "greet.sh").replace('\\', "/");
    let diff = format!(
        "--- a/{path}\n\
         +++ b/{path}\n\
         @@ -5,3 +5,3 @@\n\
         \x20for name in \"$@\"; do\n\
         -  echo \"Hello, $name\"\n\
         +  echo \"Hello, $name!\"\n\
         \x20done\n"
    );
    apply_ok("apply_patch", &json!({ "diff": diff }));
    let c = read(&wd, "greet.sh");
    assert!(
        c.contains("echo \"Hello, $name!\""),
        "F1: fix not applied:\n{c}"
    );
}

#[test]
fn edit_tools_reject_unsafe_edits() {
    // These guards are what small local models rely on to avoid edit spirals;
    // Wave B's edit-tool collapse must preserve every one of them.
    let wd = prepare("A1");
    let p = abs(&wd, "src/lib.rs");

    // 1. old_text not present → clear error, nothing written.
    let err = dispatch_tool(
        "edit_file",
        &json!({ "path": p, "old_text": "not_in_the_file_zzz", "new_text": "x" }).to_string(),
    )
    .expect_err("missing old_text must error");
    assert!(err.contains("not found"), "missing-text error: {err}");

    // 2. no-op edit (old == new, unique match) → loud failure, not a false ok.
    let err = dispatch_tool(
        "edit_file",
        &json!({
            "path": p,
            "old_text": "arithmetic mean of the given values",
            "new_text": "arithmetic mean of the given values",
        })
        .to_string(),
    )
    .expect_err("no-op edit must error");
    let lower = err.to_lowercase();
    assert!(
        lower.contains("no change") || lower.contains("identical"),
        "no-op guard error: {err}"
    );

    // 3. ambiguous match (>1 occurrence, no replace_all) → refuse.
    let err = dispatch_tool(
        "edit_file",
        &json!({ "path": p, "old_text": "xs", "new_text": "ys" }).to_string(),
    )
    .expect_err("ambiguous edit must error");
    assert!(
        err.contains("appears") && err.contains("times"),
        "ambiguous-match guard error: {err}"
    );
}
