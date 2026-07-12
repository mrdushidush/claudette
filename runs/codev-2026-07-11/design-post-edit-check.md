# W4 design — post-edit check loop (opt-in, default OFF)

**One sentence:** after a successful write-class tool call (`write_file` /
`edit_file` / `apply_diff` / `apply_patch`), run a fast auto-detected syntax/type
check scoped to the changed file; on non-zero exit, append truncated check output
to that tool's result in the same turn so the model fixes breakage immediately
instead of discovering it at `run_tests` time.

**Ship shape:** zero new tools · knob `CLAUDETTE_POST_EDIT_CHECK=1` (default OFF —
knob-off must be byte-identical to today) · behavioral when ON → A/B gate before
merge · default-ON is a separate later David decision.

## Reuse before build (grounded 2026-07-11)

`tools/quality.rs` already owns every primitive this needs:
`detect_framework(&cwd)` (Cargo.toml / package.json / pytest.ini+pyproject / go.mod),
`run_command_with_timeout`, `tail()` truncation, and the offline-refusal precedent
(roast 2026-06-30 H1 / #154: toolchain shell-outs are an unguardable egress vector —
skipped under `--offline`). The new module (`tools/post_edit_check.rs` or
`quality::post_edit`) composes these; it does NOT invent a second framework prober.

## Hook point

`AgentToolExecutor::execute` post-dispatch (executor.rs): when the dispatched tool
is write-class (same set as `conversation.rs::is_mutation_tool`), the result is
`Ok`, and the knob is ON → run the check, and on failure append a
`"post_edit_check"` field to the tool-result JSON. Exact line pinned at card time.

## Detection table (v1 — by changed-file extension, cheapest credible check)

| ext | check command | scope | timeout behavior |
|---|---|---|---|
| `.rs` | `cargo check --message-format=short` | project (cargo has no single-file mode; incremental keeps it fast) | skip silently on timeout |
| `.py` | `ruff check <file>` if `ruff` on PATH, else `python -m py_compile <file>` | file | same |
| `.go` | `go vet <dir-of-file>` | package | same |
| `.js` `.mjs` `.cjs` | `node --check <file>` | file | same |
| `.ts` `.tsx` | **none in v1** — tsc is slow + tsconfig-dependent; explicitly out of scope | — | — |
| anything else | no check — zero cost | — | — |

`CLAUDETTE_CHECK_CMD` override: replaces the whole table. If it contains `{file}`,
substitute the changed path; otherwise append the path as the final arg. Runs from
the workspace root.

## Guardrails (the spiral-risk analysis)

The 2026-06 spiral evidence says lint noise can spin a small model. Five fences:

1. **Opt-in, default OFF.** Absent knob → the code path is a single env check that
   returns early; wire bytes identical to today.
2. **Silence on success.** Exit 0 → append NOTHING. The feature only ever speaks
   when the edit broke something.
3. **Head-truncation:** first 30 lines / 2000 chars of combined output (compilers
   put the primary error first; `tail` would show the least useful lines).
4. **Fix-round cap (`CLAUDETTE_CHECK_MAX_ROUNDS`, default 2):** after the same
   file fails its check twice in one turn, further failures append one line —
   `"post_edit_check": "still failing (output suppressed after 2 rounds this
   turn — run run_tests or diagnostics for the full picture)"` — so a stubborn
   error can't feed an edit-check-edit-check loop.
5. **Timeout-capped** (`CLAUDETTE_CHECK_TIMEOUT_SECS`, default 10, clamp 1–120);
   timeout = silent skip (advisory, never an error, mirroring BuildTestOutcome's
   infrastructure-problem rule).

## Offline / air-gap interaction

Checks are local subprocesses BUT `cargo check` can fetch dependencies and every
toolchain executes arbitrary project code — exactly the H1 vector. Rule:
`crate::egress::is_offline()` → the whole feature is a no-op (consistent with
run_tests / diagnostics / forge Verifier). No carve-outs in v1.

## A/B plan

- Knob-OFF: assert byte-identical behavior (unit: env unset → executor result
  unchanged; plus normal battery run = regression as usual).
- Knob-ON: full brain100 A/B on BOTH gate models (qwen3.5-4b + byteshape champion)
  per goal-doc Appendix C, vs the codev-run baselines. Hypothesis: B/F-series
  (build/fix) tasks improve or hold; watch for new I-series timeouts (check
  latency) and any spiral signature in 4b logs.

## Card split (Claudette-forge, one PR each)

1. **Card W4a — pure module:** detection table + command construction +
   truncation + caps as pure functions, full unit tests (per-language fixtures,
   timeout, truncation, override parsing, offline no-op). No executor wiring.
2. **Card W4b — wiring + knobs:** executor hook, env knobs, integration tests
   (knob-off identity test included). Depends on W4a merged.

(Two cards, not three — the detection table isn't big enough to be its own PR.)
