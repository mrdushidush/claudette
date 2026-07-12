**FOLLOW-UP (attempt 2 of 2) — 4 of 5 files are already edited in the working tree. Finish the remaining work, run the gate, fix fallout, commit.**

Context: the previous mission already made these edits (verify, don't redo): `crates/claudette/src/tools/post_edit_check.rs` (dead_code allow removed; MAX_ROUNDS_ENV + max_rounds() + suppressed_notice() + 2 unit tests added), `crates/claudette/src/tools.rs` (`pub(crate) mod post_edit_check;`), `crates/claudette/src/runtime/conversation.rs` (check_fails HashMap + the post-edit-check block + 3 integration tests), `crates/claudette/src/main.rs` (3 ALLOW entries removed). It ran out of iterations before the docs file and the gate.

Remaining steps — do exactly these:

1. In `docs/configuration.md`, add a subsection under the environment-variable reference (match the surrounding heading style) titled `### Post-edit checks (opt-in)` documenting: `CLAUDETTE_POST_EDIT_CHECK` (enable; default off — feature does nothing unless set to 1/true/yes/on), `CLAUDETTE_CHECK_CMD` (custom check command; a `{file}` placeholder is substituted with the edited file's path, otherwise the path is appended as the last argument), `CLAUDETTE_CHECK_TIMEOUT_SECS` (default 10, clamped 1–120; a timed-out check is silently skipped), `CLAUDETTE_CHECK_MAX_ROUNDS` (default 2, clamped 1–10; per file per turn, further failures are summarized in one line). Also state: auto-detection when no override — `.rs` → `cargo check --message-format=short`, `.py` → `ruff check` (fallback `python -m py_compile`), `.go` → `go vet`, `.js`/`.mjs`/`.cjs` → `node --check`; success appends nothing; the whole feature is a no-op under `--offline`/`CLAUDETTE_OFFLINE`; `apply_patch` is not covered in v1.

2. Run the gate: `cargo fmt --all && cargo clippy --all-targets --all-features --no-deps -- -D warnings && cargo test && cargo test --all-features`. Fix ONLY what the gate flags (compile errors, dead_code fallout, test failures in the code the previous attempt wrote, the `every_env_var_is_documented` guard). Keep fixes minimal and within the five named files.

3. Commit — add ONLY these five files (never `claudette-writecode-*.sh`): `crates/claudette/src/tools/post_edit_check.rs`, `crates/claudette/src/tools.rs`, `crates/claudette/src/runtime/conversation.rs`, `crates/claudette/src/main.rs`, `docs/configuration.md`. Message (exactly, no Co-Authored-By trailer):
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
4. Do NOT push, do NOT open a PR. After the commit, STOP.

**Do NOT touch:** `runs/eval-2026-05-29/battery/MODEL-COMPARISON.md`, `tests/results_*`, `tests/*_prompts.txt`, any file not named above.
