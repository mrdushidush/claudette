**Task:** Wire the (already-merged, currently dead) post-edit-check module into the tool-result path, opt-in via `CLAUDETTE_POST_EDIT_CHECK` (default OFF — knob-off behavior must be byte-identical), with a per-turn fix-round cap, docs, and integration tests. Design: `runs/codev-2026-07-11/design-post-edit-check.md`. BEHAVIORAL WHEN ON → merges only after the A/B battery gate (run by the operator, not you).

Numbered steps — follow exactly:

1. In `crates/claudette/src/tools/post_edit_check.rs`:
   - Delete the line `#![allow(dead_code)] // wired into the executor in the follow-up PR (W4b)`.
   - Add after `TIMEOUT_ENV`: `pub(crate) const MAX_ROUNDS_ENV: &str = "CLAUDETTE_CHECK_MAX_ROUNDS";`
   - Add a fn (doc comment: per-file per-turn cap on appended check failures): `pub(crate) fn max_rounds() -> u32` — parse `MAX_ROUNDS_ENV`, clamp `1..=10`, default `2` when unset/unparseable.
   - Add a fn: `pub(crate) fn suppressed_notice(path: &str) -> String` returning exactly: `format!("\n\n[post_edit_check] {path} still fails its check (output suppressed after repeated rounds this turn — run run_tests or diagnostics for the full picture)")`
   - Add unit tests `max_rounds_defaults_and_clamps` (unset → 2; `"1"` → 1; `"0"` → 1; `"99"` → 10; garbage → 2; hold ENV_LOCK) and `suppressed_notice_names_the_file`.
   - If anything else in the module now fires `dead_code` under `-D warnings`, mark ONLY those items with `#[allow(dead_code)]` + a one-line reason (do not re-add the module-wide allow).

2. In `crates/claudette/src/runtime/conversation.rs`, inside the `PermissionOutcome::Allow` branch, directly AFTER the read-loop-breaker block (`if read_loop_enabled && tool_name == "read_file" && !is_error { … }`) and BEFORE the no-progress counter block, insert:
   ```rust
   // Post-edit check (opt-in, CLAUDETTE_POST_EDIT_CHECK): after a
   // successful single-file write, run a fast syntax/type check and
   // surface failures in the same tool result so the brain fixes
   // breakage now, not at run_tests time. apply_patch is excluded in
   // v1 (multi-file). Capped per file per turn so a stubborn error
   // can't feed an edit↔check spiral (design 2026-07-11, W4).
   if !is_error
       && matches!(tool_name.as_str(), "write_file" | "edit_file" | "apply_diff")
       && crate::tools::post_edit_check::enabled()
   {
       let path = read_file_path(&input);
       if !path.is_empty() {
           let workspace = crate::missions::active_cwd();
           if let Some(check) = crate::tools::post_edit_check::run_post_edit_check(
               std::path::Path::new(&path),
               &workspace,
           ) {
               let rounds = {
                   let n = check_fails.entry(path.clone()).or_insert(0);
                   *n += 1;
                   *n
               };
               if rounds <= crate::tools::post_edit_check::max_rounds() {
                   output.push_str(&format!(
                       "\n\n[post_edit_check] the edited file fails its check:\n{check}\nFix this before moving on."
                   ));
               } else {
                   output.push_str(&crate::tools::post_edit_check::suppressed_notice(&path));
               }
           }
       }
   }
   ```
   - Declare `let mut check_fails: std::collections::HashMap<String, u32> = std::collections::HashMap::new();` in the same scope where `read_seen` is declared (per-turn-loop state), adjusting to that scope's actual lifetime (it must reset where `read_seen` resets).
   - `read_file_path` already exists in this file (~line 1068) and parses the `"path"` field — reuse it; do NOT write a new parser. If its current name/visibility doesn't fit, adapt the call, not the helper.
   - In `crates/claudette/src/tools.rs`, change `mod post_edit_check;` to `pub(crate) mod post_edit_check;` so conversation.rs can reach it.

3. In `crates/claudette/src/main.rs`: remove the three ALLOW entries `"CLAUDETTE_POST_EDIT_CHECK"`, `"CLAUDETTE_CHECK_CMD"`, `"CLAUDETTE_CHECK_TIMEOUT_SECS"` and their 3-line comment from `every_env_var_is_documented` (the vars become documented in step 4; the guard must pass WITHOUT the allows).

4. In `docs/configuration.md`, add a subsection under the environment-variable reference (match the surrounding heading style) titled `### Post-edit checks (opt-in)` documenting all four vars: `CLAUDETTE_POST_EDIT_CHECK` (enable, default off), `CLAUDETTE_CHECK_CMD` (override, `{file}` placeholder else path appended), `CLAUDETTE_CHECK_TIMEOUT_SECS` (default 10, clamp 1–120), `CLAUDETTE_CHECK_MAX_ROUNDS` (default 2, clamp 1–10). State: auto-detection table (`.rs` cargo check / `.py` ruff-or-py_compile / `.go` go vet / `.js|.mjs|.cjs` node --check), silence on success, skipped entirely under `--offline`, `apply_patch` not covered in v1.

5. Integration tests in `runtime/conversation.rs`'s existing `#[cfg(test)]` mod, following its existing stub-executor + env-handling patterns exactly:
   - `post_edit_check_off_leaves_tool_result_untouched` — knob unset; a stub write_file turn's tool_result body contains no `[post_edit_check]`.
   - `post_edit_check_appends_failure_output` — knob on + `CLAUDETTE_CHECK_CMD` set to `git definitely-not-a-subcommand {file}` (portable non-zero exit); body contains `[post_edit_check]`.
   - `post_edit_check_caps_rounds_per_file` — knob on, same failing setup, `CLAUDETTE_CHECK_MAX_ROUNDS=1`, TWO write_file calls to the same path in one turn: first body has full output, second has the suppressed notice.
   (Use whatever env-lock mechanism that test mod already uses; if none, add one local static as in post_edit_check.rs. Always restore env before releasing.)

6. Touch NOTHING else.

**Do NOT touch:** `runs/eval-2026-05-29/battery/MODEL-COMPARISON.md`, `tests/results_*`, `tests/*_prompts.txt`, any file not named above (this task touches exactly FIVE files: `post_edit_check.rs`, `conversation.rs`, `tools.rs` (one word), `main.rs` (ALLOW removal), `docs/configuration.md`).

**Gate (run before finishing):** `cargo fmt --all && cargo clippy --all-targets --all-features --no-deps -- -D warnings && cargo test && cargo test --all-features` — all must pass.

**After the gate passes, COMMIT yourself** (the Verifier reads committed diffs) — add ONLY the five files above (never `claudette-writecode-*.sh`), **do NOT push, do NOT open a PR.** Commit message (exactly, no Co-Authored-By trailer):
```
feat(runtime): wire opt-in post-edit checks into write-tool results

Second half of the post-edit check loop (design:
runs/codev-2026-07-11/design-post-edit-check.md). With
CLAUDETTE_POST_EDIT_CHECK=1, a successful write_file/edit_file/apply_diff
triggers the module's auto-detected check; non-zero output is appended
to the same tool result (capped per file per turn by
CLAUDETTE_CHECK_MAX_ROUNDS, default 2). Knob off (the default) leaves
every byte of behavior unchanged. Knobs documented in configuration.md
and removed from the doc-drift ALLOW list; apply_patch excluded in v1.
```

**Expected diff:** 5 files, roughly +130/−12 lines.

After the commit, STOP.
