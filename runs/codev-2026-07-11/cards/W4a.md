**Task:** Create the pure post-edit-check module — detection table, env knobs, output truncation, and check runner — with full unit tests. NOTHING calls this module yet (wiring is a separate follow-up PR), so this change is behavior-preserving by construction.

Numbered steps — follow exactly:

1. Create `crates/claudette/src/tools/post_edit_check.rs`. Start the file with a module doc comment explaining: after a successful write-class tool call, `run_post_edit_check` runs a fast auto-detected syntax/type check against the changed file and returns truncated failure output (None on success/skip); opt-in via `CLAUDETTE_POST_EDIT_CHECK`, and a no-op under offline mode because check toolchains execute arbitrary project code (same rule as `run_tests` — roast 2026-06-30 H1). Immediately after the module doc add:
   ```rust
   #![allow(dead_code)] // wired into the executor in the follow-up PR (W4b)
   ```

2. Implement, in this order, with these EXACT names, signatures, and behaviors:
   - `pub(crate) const CHECK_ENV: &str = "CLAUDETTE_POST_EDIT_CHECK";`
   - `pub(crate) const CMD_ENV: &str = "CLAUDETTE_CHECK_CMD";`
   - `pub(crate) const TIMEOUT_ENV: &str = "CLAUDETTE_CHECK_TIMEOUT_SECS";`
   - `pub(crate) const MAX_OUTPUT_LINES: usize = 30;`
   - `pub(crate) const MAX_OUTPUT_CHARS: usize = 2000;`
   - `pub(crate) struct CheckCmd { pub program: String, pub args: Vec<String>, pub cwd: std::path::PathBuf }` with `#[derive(Debug, Clone, PartialEq, Eq)]`.
   - `pub(crate) fn enabled() -> bool` — true iff `CHECK_ENV` is set to `1`, `true`, `yes`, or `on` (ASCII case-insensitive, trimmed). Unset/anything else → false (default OFF).
   - `pub(crate) fn timeout_secs() -> u64` — parse `TIMEOUT_ENV`, clamp to `1..=120`, default `10` when unset/unparseable.
   - `pub(crate) fn override_cmd(raw: &str, file: &std::path::Path, workspace: &std::path::Path) -> Option<CheckCmd>` — split `raw` on whitespace; empty → `None`. First token = program, rest = args. Every arg containing the literal `{file}` gets it replaced by `file.display()`. If NO token contained `{file}`, append `file.display()` as one extra final arg. `cwd` = workspace.
   - `pub(crate) fn builtin_cmd(file: &std::path::Path, workspace: &std::path::Path, ruff_available: bool) -> Option<CheckCmd>` — match on the file's extension (lowercased):
     - `rs` → program `cargo`, args `["check", "--message-format=short"]`, cwd workspace
     - `py` → if `ruff_available`: `ruff` + `["check", <file>]`; else `python` + `["-m", "py_compile", <file>]`; cwd workspace
     - `go` → program `go`, args `["vet", "."]`, cwd = the file's parent directory (fall back to workspace if it has none)
     - `js` | `mjs` | `cjs` → program `node`, args `["--check", <file>]`, cwd workspace
     - anything else (including `ts`/`tsx` — deliberately unsupported in v1) → `None`
   - `pub(crate) fn ruff_on_path() -> bool` — `std::process::Command::new("ruff").arg("--version")` output; `Ok` with success status → true, any error/non-zero → false.
   - `pub(crate) fn truncate_output(raw: &str) -> String` — take the FIRST `MAX_OUTPUT_LINES` lines (head, not tail — compilers print the primary error first), rejoin with `\n`, then if the result exceeds `MAX_OUTPUT_CHARS` cut at a char boundary; if anything was dropped in either step, append `\n… (check output truncated)`.
   - `pub(crate) fn command_for(file: &std::path::Path, workspace: &std::path::Path) -> Option<CheckCmd>` — if `CMD_ENV` is set and non-empty after trim → `override_cmd`; else `builtin_cmd(file, workspace, ruff_on_path())`.
   - `pub(crate) fn run_post_edit_check(file: &std::path::Path, workspace: &std::path::Path) -> Option<String>` — early-return `None` unless `enabled()`; `None` if `crate::egress::is_offline()`; `None` if `command_for` is `None`. Otherwise run via `crate::test_runner::run_command_with_timeout(&cmd.program, &args, timeout_secs(), Some(&cmd.cwd))` (build `args: Vec<&str>` from the `CheckCmd`; check that helper's exact signature and adapt the call, it is already `pub`). `timed_out` → `None` (advisory skip). `exit_code == 0` → `None` (silence on success). Otherwise `Some(truncate_output(&format!("{}\n{}", stdout, stderr)))`.

3. Add a `#[cfg(test)] mod tests` in the same file with its OWN `static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());` (every test that touches env vars must hold it, and must restore/remove the vars before dropping the guard). Implement EXACTLY these tests:
   - `enabled_defaults_off_and_parses_truthy` — unset → false; `"1"`, `"true"`, `"YES"`, `"on"` → true; `"0"`, `"off"`, `""` → false.
   - `timeout_defaults_and_clamps` — unset → 10; `"5"` → 5; `"0"` → 1; `"999"` → 120; `"garbage"` → 10.
   - `builtin_cmd_rust_maps_to_cargo_check`
   - `builtin_cmd_python_prefers_ruff_falls_back_py_compile` — both values of `ruff_available`.
   - `builtin_cmd_go_targets_package_dir`
   - `builtin_cmd_js_node_check` — all three of `js`/`mjs`/`cjs`.
   - `builtin_cmd_unknown_ext_is_none` — `.ts`, `.tsx`, `.md`, `.toml`, and an extensionless path.
   - `override_cmd_substitutes_file_placeholder`
   - `override_cmd_appends_file_when_no_placeholder`
   - `override_cmd_empty_is_none`
   - `truncate_output_passthrough_when_small`
   - `truncate_output_caps_lines_then_chars` — 40 one-char lines → 30 lines + marker; a single 5000-char line → ≤ MAX_OUTPUT_CHARS + marker.
   - `run_post_edit_check_disabled_returns_none` — with `CHECK_ENV` unset the fn returns `None` (and must not spawn anything — use a `CMD_ENV` pointing at a nonexistent program to prove it was never run).
   - `run_post_edit_check_skips_under_offline` — `CHECK_ENV=1` + `CLAUDETTE_OFFLINE=1` → `None`.

4. Register the module: in `crates/claudette/src/tools.rs`, add `mod post_edit_check;` in alphabetical order among the existing `mod` declarations (between `mod patch;` and `pub(crate) mod quality;`).

5. Touch NOTHING else.

**Do NOT touch:**
- `runs/eval-2026-05-29/battery/MODEL-COMPARISON.md`
- any file under `tests/results_*`
- any `tests/*_prompts.txt`
- any file not named above (this task touches exactly TWO files: the new `post_edit_check.rs` and the one-line mod registration in `tools.rs`).

**Gate (run before finishing):** `cargo fmt --all && cargo clippy --all-targets --all-features --no-deps -- -D warnings && cargo test && cargo test --all-features` — all must pass.

**After the gate passes, COMMIT the changes yourself** (the Verifier reads committed diffs), **but do NOT push and do NOT open a PR** — the operator pushes the mission branch and opens the PR. Commit message (exactly, no Co-Authored-By trailer):
```
feat(tools): pure post-edit-check module (detection, knobs, truncation)

First half of the opt-in post-edit check loop (design:
runs/codev-2026-07-11/design-post-edit-check.md). Pure functions only —
extension→check detection (cargo check / ruff / go vet / node --check),
CLAUDETTE_CHECK_CMD override with {file} substitution, first-30-lines
truncation, 10s clamped timeout, silence-on-success, offline no-op.
Nothing is wired into the executor yet, so behavior is unchanged; the
executor hook + knob plumbing land in the follow-up PR.
```

**Expected diff:** 2 files — `crates/claudette/src/tools/post_edit_check.rs` (new, roughly 250–350 lines incl. tests) and `crates/claudette/src/tools.rs` (+1 line).

After the gate passes, STOP — report done and let the pipeline submit.
