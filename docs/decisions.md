# Architecture Decisions

Convention: AD-N sequential. Each captures *problem → options → decision → consequences*. Nice-to-have documentation for load-bearing choices; readers can go straight from "why does it work this way" to the rationale without asking.

---

## AD-1 — Multi-crate cargo workspace (2026-04-22)

**Problem.** Single-crate vs workspace-with-member-crates vs polyrepo for the v0.1 structure.

**Options considered.**

- (a) Single super-crate, like claudette's current discipline (30K Rust LOC in one crate).
- (b) Cargo workspace with member crates, single binary, monorepo.
- (c) Polyrepo, one git repo per crate.

**Decision.** Option (b). Monorepo cargo workspace with six library crates + one binary crate.

**Rationale.**

- **Verifier is standalone-usable** against arbitrary repos (via `claudette-verify <path>`). Easiest to publish independently with its own crate; also gives future users the option to `cargo install claudettes-forge-verifier` without pulling forge / integrations.
- **Integrations** (telegram / voice / mcp) live behind feature flags. Feature flags are clean at crate boundaries.
- **Precedent.** claudette hit 30K LOC in a single crate and did a 14-way tool-module split mid-development (commit log shows `tools.rs` went 4,821 → 1,184 LOC). Starting as a workspace avoids that pain.
- **Polyrepo rejected.** Cross-crate refactoring is harder, CI multiplies, changelog fragments. The user is solo and values velocity.

**Consequences.**

- Slightly heavier boilerplate (a Cargo.toml per crate).
- At v0.0.1 several crates are near-empty (intentional — they get filled over Sprints 1-5).
- Crate names use `claudettes-forge-<role>` prefix so we can publish individual crates to crates.io without collisions. The main binary stays as `claudettes-forge`.

---

## AD-2 — Assistant + forge in one binary (2026-04-22)

**Problem.** Ship two products (claudette-style secretary + BCF-style coder) or one product with two modes?

**Decision.** One binary, two modes. `claudettes-forge` in no-args runs assistant mode; `claudettes-forge forge <mission>` runs the pipeline.

**Rationale.**

- Shared substrate: personas, TUI, permissions, OAuth, memory don't fork.
- Progressive reveal: users onboard via the assistant and graduate to forge when they're ready.
- Single binary + single install + single slash-command namespace.
- claudette already had tools-as-first-class, which makes forge mode a natural extension rather than a parallel universe.

**Consequences.**

- Users who want *only* the verifier can still use `cargo install claudettes-forge-verifier` for the standalone binary.
- Binary size grows with feature flags (voice adds .wav banks, LanceDB if eventually included adds dependencies). Default-off flags keep the baseline small.

---

## AD-3 — Ollama-first at v0.1, Anthropic added v0.2 (2026-04-22)

**Problem.** Default provider policy.

**Decision.** Ollama-only at v0.1. Anthropic Claude added at v0.2 as the only cloud option.

**Rationale.**

- claudette shipped Ollama-only and proved the assistant-mode hypothesis.
- Archive/overnight-run-3 `results.csv` shows local `qwen3-coder-next:q8_0` matching or beating `opus` + `sonnet` on 10/10 benchmark templates (mean 8.5 vs 7.8 / 7.7). Local-first is defensible, not a compromise.
- Shipping cloud support later avoids API-key UX + billing surfaces + rate-limit handling in v0.1.

**Consequences.**

- Users without Ollama cannot use v0.1. A `claudettes-forge doctor` check should detect this and point at install docs.
- v0.2 adds native `tool_use` for Claude with a text-fallback path for degraded cases.

---

## AD-4 — 7-stage forge pipeline with surgical-by-default fix-loop (2026-04-22)

**Problem.** Stage count for forge-mode pipeline.

**Decision.** 7 stages: `Router → Planner → Coder → TestCoder → Verifier → SurgicalCoder → Gate`. The Verifier implicitly loops back to SurgicalCoder until Gate passes or max-rounds exceeded. Fix policy: surgical by default; full regen only at round 1 when score<8.5 AND compile failed.

**Rationale.**

- stealthsambaV2's 10-stage added Spec-Fidelity (3b) and Independence (8) as separate stages — both useful concepts but deferred as opt-ins rather than always-on baseline.
- BCF's 9-stage included Security + Critique + CTO — folded into Gate's checklist.
- Surgical > regen is the strongest convergent learning in the family ("full regen always degrades score," BCF learning #12). Default enforces the empirically-correct behaviour.

**Consequences.**

- `--pro` flag could eventually add Security / Critique / CTO / Independence stages back as additional stages (post-v0.2).
- Double-Context Phase-0 is a Coder-stage internal behaviour, not a separate stage — keeps the stage count honest at 7.

---

## AD-5 — 5-tier permissions model from claw-code (2026-04-22)

**Problem.** Permission tier count.

**Decision.** 5-tier from claw-code upstream: `ReadOnly` / `WorkspaceWrite` / `DangerFullAccess` / `Prompt` / `Allow`. `PermissionPolicy::authorize()` trait with a swappable `PermissionPrompter` (v0.1 default: TTY modal).

**Rationale.**

- Claw-code's model is the richest in the family and well-understood by any user who's used Claude Code (the upstream). Familiarity is a ship advantage.
- Provenance-wrapping pattern (extending claudette's `<email>` defanging to web-fetch / calendar / gmail bodies) layers *on top* of this model, not replacing it.

**Consequences.**

- More surface area than claudette's 3-tier, but the extra two tiers (`Prompt`, `Allow`) are useful for interactive sessions where the user wants per-call confirmation without an ambient "I'm in dev mode" global.
- Platform sandboxing (macOS `sandbox-exec`, Linux `bwrap`, Windows TBD) wraps `DangerFullAccess` at the process layer as defense-in-depth.

---

## AD-6 — Toolchain + lint baseline (2026-04-24, pre-Sprint-3 audit)

**Problem.** Three separate toolchain/hygiene choices surfaced during the pre-Sprint-3 audit and needed explicit spec before starting Sprint 3:

1. **MSRV was fictional.** `rust-version = "1.75"` had been in the workspace `Cargo.toml` since scaffolding, but `reqwest 0.12` (Sprint 1 session 4) transitively pulls `indexmap 2.14` which requires `edition2024` — unavailable before Rust 1.85. Anyone actually trying to build on 1.75 hit `feature edition2024 is required` before a single line of our code ran.
2. **Clippy pedantic was claimed workspace-wide, applied to `core` only.** Sprint 2 exit criteria said "Clippy pedantic green". Only `crates/core/src/lib.rs` has `#![warn(clippy::pedantic)]`; the other six crates run default clippy. Sprint 2 shipped with an unacknowledged quality gap.
3. **No line-ending enforcement.** A fresh `git clone` on Windows (`core.autocrlf=true` is the Git-for-Windows default) turned committed LF files into CRLF, which broke the persona frontmatter parser silently. Working trees with Write-tool-written files masked the bug.

**Options considered.**

- (a) Leave MSRV at 1.75, document it as aspirational. Rejected — declaring a MSRV that cannot build is a bug magnet for contributors.
- (b) Bump MSRV to the highest stable (1.95). Rejected — gratuitous; excludes users on recent-but-not-bleeding-edge toolchains for no concrete gain.
- (c) Bump to the honest minimum (1.85, forced by `edition2024` in the dep tree). **Selected** for MSRV.
- (d) Bump to 1.87 to regain `u64::is_multiple_of`. Rejected — buys back one convenience method at the cost of two extra Rust versions of buffer; the `% == 0` alternative is already in place with a scoped `#[allow]`.
- (e) Accept the Sprint 2 claim and silently tighten only when it breaks. Rejected for clippy — the gap was real, and Sprint 3 is the right moment to correct it before another crate lands with relaxed lints.
- (f) Rely on parser robustness alone (CRLF-tolerant frontmatter) without `.gitattributes`. Rejected — we want both belts and braces; future parsers shouldn't have to repeat the CRLF-normalise workaround.

**Decision.**

1. **MSRV = 1.85.** The honest minimum that the current dep graph can actually build on. `Cargo.toml::workspace.package.rust-version = "1.85"`. Sprint 3's hard constraint updates to match.
2. **Clippy pedantic enabled workspace-wide as a Sprint 3 checklist item.** Every member crate adds `#![warn(clippy::pedantic)]` to its lib/bin root and fixes the resulting warnings (roughly 88 across tui + binary at audit time). `clippy::module_name_repetitions` stays allowed workspace-wide because of our `claudettes_forge_*` naming convention.
3. **`.gitattributes` with `* text=auto eol=lf`** committed at the repo root. Binary file globs (`*.png`, `*.wav`, etc.) marked `binary` so git never touches them. Parsers that consume text files from disk (personas today, additional loaders in Sprint 3) should still CRLF-normalise in code as defense-in-depth.

**Rationale.**

- **1.85 is the oldest Rust that actually works.** Going lower is a lie; going higher is unnecessary.
- **Uniform pedantic in a seven-crate workspace prevents drift.** With a single `core` crate under pedantic, every PR that touches `tui` or `forge` silently gets a lower bar, which is how Sprint 2 shipped with a docstring claiming a trait bound that wasn't real. Pedantic workspace-wide catches those cheaply.
- **`.gitattributes` short-circuits the CRLF issue at checkout.** Parser-side normalisation is a safety net, but enforcing LF on checkout means every contributor sees the same bytes regardless of git config.

**Consequences.**

- Users on Rust 1.75–1.84 cannot build. In practice this is zero users: Sprint 1 and 2 never actually worked on those versions.
- Sprint 3 carries a pre-flight "enable pedantic on tui/binary/forge/verifier/integrations/bench, fix warnings" item before any new feature work. Budget ~2–4 hours; most of the warnings are `must_use`, `cast_possible_truncation`, `too_many_lines`, and parameter-passing-style nits.
- Contributors with existing clones on `core.autocrlf=true` may see a one-time bulk re-checkout when `.gitattributes` lands — `git rm --cached -r . && git checkout .` after a pull resynchronises line endings. Documented in Sprint 3 onboarding notes.
- This ADR supersedes the Sprint 2 hard constraint "MSRV is 1.75" (historical record kept intact in `docs/sprints/sprint_02_tui.md`).

---

## AD-7 — Forge pipeline architecture + benchmarking-driven gate (2026-04-24, Sprint 3 kickoff)

**Problem.** Sprint 3 kickoff surfaced five choices that shape the `forge` crate port from BCF-ABCC. Making them implicitly during mid-checklist commits would fragment the decision history; batching into one ADR lets Sonnet start at step 1 without re-litigating.

1. **TestCoder / Coder ordering.** AD-4 locked the sequence `Router → Planner → Coder → TestCoder → Verifier → SurgicalCoder → Gate`, but BCF-ABCC's 10-mission stress test showed TDD-first (tests before code) scoring ~1.7 points higher on average with an Opus tester + local coder setup. Do we reorder to TDD or keep AD-4 literal?
2. **Gate collapse strategy.** AD-4 folds BCF-ABCC's Security + Critique + CTO + QualityGate into a single `Gate`. Execute as one structured-JSON LLM call, or as three internal sub-stages sequenced under a single gate boundary?
3. **Pipeline concurrency.** BCF-ABCC is tokio-first. Sprint 1/2 are 100% sync (blocking `reqwest`, threads + `std::sync::mpsc`). Does the `forge` crate introduce `tokio` to the workspace?
4. **`models.toml` schema evolution.** Sprint 2 shipped a minimal `role → model` schema. BCF-ABCC runs 8 per-role configs with `context_size` + `max_predict`. How do we extend without breaking Sprint 2 files?
5. **Quality gate threshold.** BCF-ABCC's complexity-scaled 9.2/8.5/8.0 gate is empirically unreachable with all-local models (10-mission average 7.5). Do we hardcode 9.2 and accept "best-round restored" on most missions, scale down to a lower default, or make it config-driven?

**Options considered.**

- (1a) Keep AD-4 literal — Coder → TestCoder. **Selected** per user preference 2026-04-24: TestCoder validates the produced code against a richer specification, rather than writing tests-first against a plan.
- (1b) Reorder to TDD — TestCoder → Coder. Rejected for Sprint 3; carries the BCF-ABCC empirical advantage (+1.7 points in the 10-mission stress test) but trades against AD-4 continuity. Revisit via AD-8 if Sprint 3 benchmarks show an unrecoverable gap.
- (2a) Gate = single structured-JSON LLM call — security verdict + 5-score critique + CTO verdict + score in one call. **Selected**. Matches BCF-ABCC learning #5 ("single critique call > 5 parallel calls, Ollama is sequential anyway"). Cost + latency win is material with Ollama.
- (2b) Gate = three internal sub-stages. Rejected for v0.2; escalation path if (2a) produces flaky judgements under benchmark.
- (3a) Keep pipeline sync. **Selected**. Sprint 2 worker demonstrates the pattern works; no pipeline stage has concurrent I/O that async would help (Ollama is sequential, fix-pass loop is sequential, stages are sequential).
- (3b) Introduce tokio to the `forge` crate. Rejected. Cross-crate async adoption is a workspace-wide concurrency retrofit and deserves its own AD; the bridge-layer complexity to reach into sync `core` primitives adds more surface than the net ergonomic gain.
- (4a) Extend `models.toml` with optional fields (`context_size`, `max_predict`, `pipeline.gate_threshold`, `pipeline.gate_preset`). Backwards-compatible; missing fields get defaults logged. **Selected**.
- (4b) Break schema and require migration. Rejected — Sprint 2 files keep working, migration deferred to v0.3 if a cleanup pass justifies it.
- (5a) Hardcode 9.2 threshold. Rejected — BCF-ABCC data shows this is unreachable on all-local model sets.
- (5b) Hardcode a lower threshold (e.g. 7.5). Rejected — numbers without the data behind them invite bikeshedding; benchmarking should drive the value.
- (5c) Make threshold config-driven with a sensible default (8.0 per user "good enough to ship" bar) + preset for aspirational-mode. **Selected**.

**Decisions.**

1. **Pipeline order follows AD-4 literal:** `Router → Planner → Coder → TestCoder → Verifier → SurgicalCoder → Gate`. TestCoder runs *after* Coder and validates against a test plan produced by Planner.
2. **Gate is a single LLM call** returning structured JSON with security verdict, 5-score critique (DEV/ARCH/TEST/SEC/DOCS), CTO verdict, and final score `critique_avg * 0.4 + verifier_score * 0.6`.
3. **Pipeline stays sync.** No `tokio`, `async fn`, or `.await` in any Sprint 3 delta. `std::thread::spawn` + `std::sync::mpsc` is the concurrency primitive. Cross-crate tokio adoption is deferred indefinitely; if it ever lands it gets its own AD.
4. **`models.toml` schema gains optional fields:** per-role `context_size` + `max_predict`; workspace-level `pipeline.gate_threshold` + `pipeline.gate_preset`. Files written for Sprint 2 continue to load — missing fields log a one-line defaults notice. Role mapping from pipeline stage: Router→`complexity`, Planner→`architect`, Coder→`coder`, TestCoder→`tester`, SurgicalCoder→`fix_coder`, Gate→`critique`. Verifier is deterministic code, not an LLM role.
5. **Quality gate threshold is config-driven.** Default: **8.0** (ship-worthy per user). Override via `models.toml::pipeline.gate_threshold` or CLI `--gate-threshold`. Preset `aspirational` enables BCF-ABCC's 9.2/8.5/8.0 Campbell-tier ladder. The default value is subject to post-Sprint-3 bench-tuning on local-only and local+cloud runs; moving the default is a follow-up commit, not a re-opening of Sprint 3.

**Rationale.**

- **AD-4 literal ordering** maintains decision continuity with the original design intent. The BCF-ABCC TDD advantage is real but unfortunately tangled with using Opus as tester; isolating the ordering effect from the model-quality effect is itself a benchmarking task that happens *after* Sprint 3 ships. If the isolated ordering effect is large, AD-8 reorders cleanly.
- **Single Gate call** extends the BCF-ABCC learning "single critique call > 5 parallel calls" from the internal critique panel to the whole gate stage. Cheaper, faster, and the LLM can self-consistently score across the dimensions (a pattern BCF-ABCC learning #5 already validates).
- **Sync discipline** preserves the bug-free concurrency model Sprint 2 shipped. Mixing sync and async crate boundaries creates footguns (blocking a tokio runtime, starving a pool, async-in-sync context panics). Staying sync avoids all of them for zero pipeline-semantics loss.
- **Schema extension over break** is the usual safer move; Sprint 2 `models.toml` was only in the wild for two days, but the discipline is worth establishing early — downstream files (user-written configs) should always be readable by later versions.
- **Config-driven gate** separates algorithm from policy. The algorithm is "score vs threshold"; the threshold is empirical. Hardcoding either 9.2 (BCF default) or 7.5 (local-only average) builds in an opinion the data hasn't been run yet to justify.

**Consequences.**

- **Order risk (D1).** If Sprint 3 benchmarks show TDD-first would score meaningfully higher, AD-8 reorders the pipeline. Sonnet should be aware this is a reopenable decision.
- **Gate refactor surface (D2).** If (2a) produces unstable judgements on curated benchmarks, gate internals refactor to three sub-stages while keeping the external `Gate` boundary. External contract (inputs / outputs / `--gate-preset` semantics) stays stable.
- **Async isolation (D3).** Any v0.3+ feature that genuinely wants async (e.g. concurrent provider calls, long-poll MCP transport) must propose its own AD and handle the cross-crate bridge explicitly.
- **Models.toml migration (D4).** Deprecation-style log line appears for every field defaulted; if the noise becomes a paper cut, suppress after one session or behind a `--quiet` flag. No hard break planned before v0.3.
- **Bench-driven threshold (D5).** Sprint 3 explicitly does not include the benchmarking work; `bench` crate body lands in Sprint 7. The 8.0 default stands until empirically refuted. The `aspirational` preset gives cloud-only users the BCF ladder on day one.


