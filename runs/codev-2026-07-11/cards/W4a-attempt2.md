**FOLLOW-UP (attempt 2 of 2) — the work is already 95% done in the working tree. Do NOT rewrite anything; make exactly one fix, run the gate, commit.**

Context: the previous mission wrote `crates/claudette/src/tools/post_edit_check.rs` (and registered `mod post_edit_check;` in `tools.rs`). The module compiles, clippy passes, and 13 of its 14 unit tests pass. ONE assertion is wrong.

1. In `crates/claudette/src/tools/post_edit_check.rs`, in the test `truncate_output_caps_lines_then_chars`, find the line:
   ```rust
   assert!(result.len() <= MAX_OUTPUT_CHARS + "… (check output truncated)".len());
   ```
   and replace it with (the marker's leading `\n` was missing from the allowance):
   ```rust
   assert!(result.len() <= MAX_OUTPUT_CHARS + "\n… (check output truncated)".len());
   ```
2. Change NOTHING else in any file.
3. Run the gate: `cargo fmt --all && cargo clippy --all-targets --all-features --no-deps -- -D warnings && cargo test && cargo test --all-features` — all must pass.
4. Commit ALL the post-edit-check changes (the module file + the `tools.rs` registration line — do NOT `git add` any `claudette-writecode-*.sh` files or anything else) with exactly this message (no Co-Authored-By trailer):
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
5. Do NOT push and do NOT open a PR. After the commit, STOP.

**Do NOT touch:** `runs/eval-2026-05-29/battery/MODEL-COMPARISON.md`, `tests/results_*`, `tests/*_prompts.txt`, any file not named above.
