# Import Sweep — D:\dev + D:\dev\abcc_projects → claudette

**Date:** 2026-05-19
**Goal:** Inventory every importable feature/pattern from sibling projects so claudette can absorb the best of the family and the rest can be archived. After this sweep, only claudette ships.

This sweep builds on `D:\dev\abcc_projects\abcc_projects\PROJECT_REVIEW.md` (the existing 2,431-line catalog of abcc_projects/), extends it to the D:\dev top-level projects that review skipped, and cross-references everything against claudette's current state (post-v0.5.4, forge mode shipped, MTP benchmarks done).

## How this report is organized

- **§1 — Already-in-claudette confirmation.** What the existing review flagged as wanted that is now live (so we don't double-count).
- **§2 — Tier-1 imports.** High-value features absent in claudette today, ready for direct port.
- **§3 — Tier-2 imports.** Worthwhile but bigger lift or design questions remain.
- **§4 — Tier-3 (consider).** Low-priority, niche, or duplicative.
- **§5 — Hard PASS.** Things the review/memory flagged as anti-patterns or superseded.
- **§6 — Per-project verdict.** What survives the sweep from each source repo.

`PROJECT_REVIEW.md` is the upstream reference for abcc_projects/ details — this doc layers on top, doesn't duplicate it.

---

## 1. Already in claudette (confirmed live)

Cross-referencing PROJECT_REVIEW.md §12 cherry-pick catalog against `crates/claudette/src/` today (post-v0.5.4):

| Feature | Status in claudette | Source |
|---|---|---|
| Direct provider SDKs (no litellm) | ✅ `api.rs` Ollama HTTP direct | inherited from life-hacker |
| `models.toml` per-role config | ✅ `model_config.rs` + `forge/models_toml.rs` | inherited |
| Surgical fix-loop (Codet) | ✅ `codet.rs` 1484 LOC | inherited |
| Branch-per-mission git isolation | ✅ `missions.rs` (mission_start/submit, marker leak fixed 5-14) | new in claudette |
| Mission isolation per UUID dir | ✅ `missions.rs` | new in claudette |
| `models.toml` named presets | ✅ Auto / Fast / Smart presets | inherited |
| `learnings.md`-style memory | ✅ `memory.rs` + CLAUDETTE.md discovery | inherited |
| Forge 3-role pipeline (Planner/Coder/Verifier) | ✅ `forge/mod.rs` (v0a/b/c shipped 5-12) | new in claudette |
| Single-binary, `#![forbid(unsafe_code)]` | ✅ | inherited |
| Conventional Commits + AD-numbered docs | ✅ throughout | inherited |
| Loopback OAuth (Calendar+Gmail) | ✅ `google_auth.rs` | inherited |
| Scope-separated OAuth tokens | ✅ separate `google_oauth*.json` files | inherited |
| Prompt-injection provenance tags | ✅ Gmail `<email>` defanging | inherited |
| Clock trait + MockClock | ✅ `clock.rs` | inherited |
| Deterministic schedule parsing (Rust validates) | ✅ `scheduler.rs` | inherited |
| Telegram mpsc single-consumer | ✅ `telegram_mode.rs` | inherited |
| Morning briefing | ✅ `briefing.rs` | inherited |
| Progressive paragraph streaming (TG) | ✅ | inherited |
| 100-prompt brain regression harness | ✅ `tests/` | inherited |
| 22 slash commands + dispatcher (TUI-reachable) | ✅ `commands.rs` (TUI dispatcher fixed 5-14) | inherited+fixed |
| Ratatui TUI 5-tab + worker thread | ✅ `tui.rs` 1693 LOC | inherited |
| Recall / embeddings | ✅ `recall.rs` + `tools/recall.rs` (startup probe + sticky-disable + `/recall reprobe`) | new in claudette |
| Image attach + token estimation | ✅ `image_attach.rs` | new in claudette |
| Compaction with tier-aware logging | ✅ `runtime/compact.rs` | new in claudette |
| MTP draft speculation | ✅ `api/harmony.rs` + MTP benchmark | new in claudette |
| Installer (`install.ps1`/`install.sh`) | ✅ | new in claudette |
| `doctor` subcommand | ✅ `doctor.rs` | new in claudette |
| PRIVACY.md + show-me.md docs | ✅ | new in claudette |
| Docker + VS Code integration | ✅ Dockerfile + `editor/` | new in claudette |
| 70+ tools across 17 groups | ✅ `tools/` | inherited |
| `markets` tools (TradingView + Algorand Vestige) | ✅ `tools/markets.rs` | inherited |

**Verdict:** Claudette has absorbed most of the assistant-mode catalog and a substantial chunk of the forge-mode catalog already. The remaining sweep targets are mostly forge enhancements, eval/bench harness, optional integrations, and persona system.

---

## 2. Tier-1 imports (recommended next)

These are well-defined features that already exist in working code (in a sibling repo) and would slot cleanly into claudette without architectural disruption.

### 2.1 Persona system (4 personas)

| | |
|---|---|
| What | First-class persona system: CodeX-7 (coder), Sentinel-9 (QA), CTO (strategic), Eva (assistant). Default-on, `--faceless` flag to disable, user-defined personas from `.claudette/personas/*.md` drop-in. |
| Source — best impl | `D:\dev\claudettes-forge\personas\*.md` (already written: codex7, sentinel9, cto, eva) + `crates/core/src/personas.rs` (500 LOC loader) |
| Verbatim backstory source | `D:\dev\agent-battle-command-center\packages\agents\src\agents\{coder,qa}.py` (godfather, intact tree) |
| Tacticode preserved copy | (was `Archive/tacticode/.../personas/codex7.py` per review — note: §14.5 says tacticode is solo so safe to lift) |
| Effort | Small. Personas files exist, loader exists, just needs wiring into claudette's runtime/prompt assembly. |
| Why now | Per §14 user locked "all personas from day 1, default-on, `--faceless` flag." This is one of the explicit clean-room decisions and it's already half-built in claudettes-forge. |

### 2.2 5-tier permission model from claw-code

| | |
|---|---|
| What | `ReadOnly` / `WorkspaceWrite` / `DangerFullAccess` / `Prompt` / `Allow` with `PermissionPolicy::authorize()` trait + swappable `PermissionPrompter`. Richer than claudette's current 3-tier. |
| Source — best impl | `D:\dev\claudettes-forge\crates\core\src\permissions.rs` (489 LOC, ready) |
| Original | `claw-code` upstream fork (per review §5) |
| Effort | Medium. Need to replace claudette's existing 3-tier in `runtime/permissions.rs` carefully — every tool call site checks this. |
| Why now | §14 J1 locked this in. The clean code is already written in claudettes-forge; lifting saves 1-2 days. |

### 2.3 SWE-bench runner

| | |
|---|---|
| What | ReAct agent loop with 7 tools, eval reports, resumable. Best-in-family benchmark harness. |
| Source — best impl | `D:\dev\clawForge\crates\forge\src\swebench.rs` (725 LOC) + `swebench_eval.rs` (87) + `swebench_tools.rs` (293) |
| Effort | Medium. Self-contained module. Wire as `claudette bench swe` subcommand. |
| Why now | The MTP benchmark (5-16) is sized for small fixtures; SWE-bench gives an industry-standard yardstick for forge mode's regression tracking. |

### 2.4 Multi-template bench harness + A/B methodology

| | |
|---|---|
| What | 10-template bench fixtures (storefront/arcade/portfolio/restaurant/dashboard/csv-analytics/log-parser/rms-scheduler/dns-parser/markdown-converter/task-queue/config-merger). A/B WITH-QA vs WITHOUT-QA, WITH-URL vs WITHOUT-URL, round-1 vs round-2 determinism reruns. Outputs `results.csv` / JSON. |
| Source | `D:\dev\clawForge\crates\forge\src\benchmark.rs` + `stress.rs` (168) + `models.rs` (235) + abcc `overnight runs/results.csv` corpus |
| Effort | Medium-large. Fixtures are the bulk; harness logic ports straight. |
| Why now | §14 K1-K4 locked "user-visible bench, A/B first-class, JSON output." Round-3 e2e sweep (5-16) is already informally A/B; formalize it. |

### 2.5 CTO chat agent (10 native tools, 5 iterations, history persistence)

| | |
|---|---|
| What | Strategic-conversation agent above the forge pipeline. 10 native tools, iteration cap, history persistence, mission-control framing. |
| Source | `D:\dev\clawForge\crates\forge\src\cto.rs` (size present, sub-300 LOC) + abcc `packages/agents/src/agents/cto.py` |
| Effort | Medium. Personas + CTO agent overlap; can be one effort. |
| Why now | Distinct from CodeX-7 (coder) — CTO is the strategic-loop hat for big missions. With forge mode shipped, this is the next natural layer. |

### 2.6 Antipattern auto-detection (failure → prompt-injection feedback loop)

| | |
|---|---|
| What | When 3 failures hit ≥70% similarity, a hard rule auto-graduates into the Engineer prompt. Closed feedback loop. Example: stealthsambaV2 Hotfix #12 sqlx-macro ban was auto-discovered. |
| Source | stealthsambaV2 (Hadar-touched — lift the *idea*, not code, per §14.5) |
| Effort | Medium. Needs a similarity comparator + a hot-prompt overlay. Claudette's memory.rs has the substrate. |
| Why now | This is the single most-distinctive emergent-knowledge pattern in the family. None of the sibling projects shipped it functioning end-to-end (review notes godfather's "self-evolving few-shots" was aspirational — opportunity to *complete* an aspiration). |

### 2.7 Best-round restore + smart stopping

| | |
|---|---|
| What | If fix rounds degrade the score, restore files from the highest-scoring round. Smart stopping: 2+ consecutive score declines → break loop. |
| Source | `D:\dev\clawForge\crates\forge\src\mission.rs` (2005 LOC) + BCF |
| Effort | Small. Add round-state tracking + restore in `forge/mod.rs` Verifier loop. |
| Why now | Forge mode currently runs N rounds with surgical fixes; one regression round can poison the result. This is the missing safety net. |

### 2.8 Double-Context Phase-0 gambit

| | |
|---|---|
| What | First attempt: same model at 2x context, single try, no retry ladder. Skip retries on pass. Cheap upside; standard ladder kicks in only on miss. |
| Source | `D:\dev\battleclaw-forge-main\rust\src\engine.rs` (MVP-readiness snapshot) |
| Effort | Small. One config knob + a short-circuit branch in the retry path. |
| Why now | §14 E3 explicitly locked this in. Currently not wired in claudette's forge. |

### 2.9 Paste-to-tempfile + typewriter effect (TUI)

| | |
|---|---|
| What | Paste >500 chars → temp file + preview; auto-cleanup on Drop. Code typewriter effect 6-12 chars/tick with corrections. |
| Source | `D:\dev\claudettes-forge\crates\tui\src\paste.rs` (146) + `typewriter.rs` (163) |
| Effort | Small. Self-contained; drop into `crates/claudette/src/tui/`. |
| Why now | TUI sweep 2026-05-17 still has §8-9 pending. Paste-tempfile is the user-favorite item from tacticode. |

### 2.10 Space Invaders easter egg

| | |
|---|---|
| What | §14 I5 said "Just Space Invaders easter egg — redesign. Snake dropped." User wants this. |
| Source | `D:\dev\claudettes-forge\crates\tui\src\space.rs` (388 LOC) |
| Effort | Small. |
| Why now | Cheap morale win; the user explicitly called it out as wanted. |

---

## 3. Tier-2 imports (worthwhile but bigger or design-deferred)

### 3.1 LanceDB + petgraph rich memory (`--memory=rich`)

- Sourced from stealthsambaV2 (Hadar-touched — lift *idea*, not code).
- §14 H1 + G4 deferred to v0.2 of the rewrite. Claudette already has `recall.rs` embeddings (lighter). Decision: keep file-backed default; LanceDB only if forge missions outgrow flat recall.
- **Defer until:** evidence that recall/embeddings can't carry the load.

### 3.2 `--independence` cross-validation

- A second-opinion Independence reviewer over the forge Verifier (stealthsambaV2 Stage 8 pattern). `independencev1` has the "7.0 floor problem" unfixed.
- §14 F1 says ship the static-analysis-clamp + LLM-review core, fix the 7.0 floor in v0.2.
- **Defer until:** forge mode has run on enough real missions that "score inflation" is measurable.

### 3.3 Multi-tenant RBAC + server mode (`--server`)

- Axum Prometheus `/metrics` on :8080, audit logs, multi-tenant token gating. Sourced from godfather/battleclaw-v2.
- §14 L1-L3 deferred. Claudette is single-user. Not on roadmap.
- **Defer indefinitely** unless someone asks for it.

### 3.4 Voice announcements + audio banks

- 192 .wav banks from godfather (`packages/ui/dist/audio/{field-command,mission-control,tactical}/`). Platform TTS dispatch (macOS `say` / Linux `espeak-ng` / Windows PowerShell TTS).
- Claudette already has `voice.rs` + `tts.rs` (Whisper STT + edge-tts TTS from life-hacker).
- §14 G7 voice = tier-1 in the rewrite; but the rewrite is the separate `claudettes-forge` repo. Claudette could absorb the .wav banks as an opt-in `--voice=mission-control` mode.
- **Decide:** Does the user want tactical voice in claudette specifically, or is that reserved for claudettes-forge?

### 3.5 Edge TTS + Whisper large-v3-turbo + Ava voice

- Already in claudette's `voice.rs` + `tts.rs` from life-hacker (per §10 review). Confirm coverage, no port needed.
- **Already covered.**

### 3.6 MCP server surface

- `clawforge-mcp` exists (`D:\dev\clawForge\crates\clawforge-mcp\src\` — protocol.rs + server.rs).
- §14 O3 deferred. Adoption signal is low (review §6).
- **Defer indefinitely** unless an external integration asks.

### 3.7 Plugin/hook runtime (claw-code style)

- claw-code defined plugin/shell hooks in config but runtime execution was missing (review §5). Building from scratch would be substantial.
- §14 O3 deferred entirely.
- **Defer indefinitely.**

### 3.8 Custom briefing templates

- Per §10 surprise #2, `BRIEFING_PROMPT` is hard-coded. Adding `--briefing --template <file>` would unlock per-user customization.
- **Small feature, do whenever briefing gets touched.**

### 3.9 Hardware monitor tab

- `D:\dev\clawForge\crates\forge\src\hardware.rs` (273 LOC). HW tab in TUI (CPU/RAM/VRAM/thermal live).
- Claudette TUI has 5 tabs (Chat/Tools/Notes/Todos/HW per review §10). **Confirm if HW tab is functional or a stub** — if stub, port from clawForge.

### 3.10 `pipeline_api` HTTP surface

- `D:\dev\clawForge\crates\forge\src\pipeline_api.rs` (388 LOC). Exposes forge as an HTTP API.
- Useful if claudette ever runs in headless server mode. Tier-3 on the audience-expansion roadmap.

### 3.11 Self-healing module

- `D:\dev\clawForge\crates\forge\src\self_healing.rs` (164 LOC). What exactly it self-heals isn't visible from the filename — needs deeper read.
- **Investigate before porting.**

### 3.12 Swarm mode (parallel multi-agent vote)

- `D:\dev\clawForge\crates\forge\src\swarm.rs` (230 LOC). C9+ parallel multi-agent execution with result voting.
- §14 E7 explicitly deferred swarm. **Defer.**

---

## 4. Tier-3 (consider only if direct user demand)

### 4.1 Trading bots' multi-DEX modules

- `algo-trading-bot/algoarb/dex/{pact,tinyman}.py` (Algorand DEXes), `algoarb/folks/` (Folks Finance lending), `algoarb/amm_math/{constant_product,price_impact}.py`, `algoarb/risk/{circuit_breaker,size_optimizer,validator}.py`.
- `sui-arb-bot/suiarb/dex/` (Sui DEXes).
- `base-liquidator/baseliq/` (Base chain liquidation).
- Claudette already has `tools/markets.rs` with TradingView + Algorand Vestige. **Niche.** Could extend `markets` to add Sui + Base if the user trades there, but the bots are Python — not a code port, more like API knowledge to encode in tool descriptions.
- **Defer** unless user wants claudette to actively monitor/execute trades (vs the current "look up prices" scope).

### 4.2 ABCC api/services TypeScript library

- `agent-battle-command-center/packages/api/src/services/` has 30+ services: `autoRetryService.ts`, `complexityAssessor.ts`, `costAggregator.ts`, `humanEscalation.ts`, `modelResolver.ts`, `ollamaOptimizer.ts`, `orchestratorService.ts`, `rateLimiter.ts`, `stuckTaskRecovery.ts`, `taskQueue.ts`, `trainingDataService.ts`, etc.
- These are TypeScript; claudette is Rust. Lift *patterns*, not code. Most concepts are already in claudette (auto-retry, cost tracking, scheduler, recall).
- **Few high-value ports remain:** `humanEscalation.ts` (when to ping the human), `trainingDataService.ts` (capture good runs for later fine-tune). Both are tier-2 if of interest.

### 4.3 Battleclaw-v2 Python validators

- `battleclawclean/python/validators/`, `tools/code_validation.py`, `tools/cto_tools.py`, `tools/shell.py` (3-layer shell security).
- 3-layer shell security (whitelist + pattern detection + language-level validation) blocks `subprocess`, `os.system` *inside* allowed commands. Claudette's `tools/shell.rs` doesn't do language-level introspection.
- **Tier-2 candidate** if forge mode starts running untrusted user prompts that pivot to shell.

### 4.4 ABCC PostgreSQL/Redis observability stack

- §14 L2-L3 + memory: deferred. Single-user assistant doesn't need it.
- **Skip.**

### 4.5 Landing page template

- `D:\dev\landing_page_grok\battleclaw-landing\` — Astro + Tailwind landing page. Useful when claudette gets a real marketing page; not code to port.
- **Defer until launch motion.**

### 4.6 Publicators-monitor

- One CSV + one PS1. Trivial. Not relevant.
- **Skip.**

### 4.7 Battle-claw skill definition

- `D:\dev\battle-claw\SKILL.md` + `clawhub.json`. Looks like a Claude Code Skill definition — for the Skill system, not Claudette code.
- **Reference only.**

### 4.8 Stealthforge_tests

- `D:\dev\stealthforge_tests\testing\` — overflow test folder from sambaV2. Per §11 it overlaps with `stealthsambaV2`. Useful as a corpus snapshot for benchmarking; not for code lift.
- **Reference only.**

---

## 5. Hard PASS (explicitly rejected or superseded)

| Item | Reason |
|---|---|
| litellm / CrewAI / MCP gateway | §14 D1: native SDKs only. Direct provider HTTP. |
| Snake easter egg | §14 I5: explicitly dropped. Space Invaders only. |
| Self-evolving few-shots (auto-append solutions ≥8) | §14 H5: dropped, "never load-bearing." Review §12.3 confirms it was aspirational — never implemented. |
| Antipattern auto-detection in v0.1 | §14 H2: deferred. (But noted Tier-1 since claudette has shipped past v0.1; reconsider.) |
| Multi-service docker-compose stack | §14 baseline: single binary. |
| Tokio everywhere | §10 + §13.2: claudette stayed single-threaded + blocking; works fine for an assistant. |
| Hadar-touched code lifts | §14.5: stealthsambaV2 / battleclaw-v2 / clawForge → lift *patterns/ideas*, not code. Use claudette + godfather + tacticode (verify solo first) as code-lift sources only. |
| 3-tier hybrid (Rust + TS + Python) | §13.4 pitfall #6: tacticode's architecture pattern explicitly rejected. |
| Web stack (React UI + Express API + CrewAI) | Godfather's Day-4 purge already removed it. |
| 7.0-floor independencev1 reviewer (as-is) | §14 F4: ships with caveat; fix is a v0.2 task. |

---

## 6. Per-project verdict

What survives the sweep from each source repo, and whether it can be archived after the relevant pieces land.

| Project | Path | Status | What's worth lifting | Archive after lift? |
|---|---|---|---|---|
| **claudettes-forge** | `D:\dev\claudettes-forge\` | Active scaffold; partial overlap with claudette | Personas + 5-tier permissions + paste.rs + typewriter.rs + space.rs (Space Invaders) + Eva persona MD | **Keep** as the rewrite-in-progress; copy the lifted modules **into claudette**, decide later if rewrite continues independently or folds back. |
| **clawForge (life-hacker)** | `D:\dev\clawForge\` | Predecessor of claudette, superseded | SWE-bench (swebench.rs / _eval / _tools), benchmark.rs harness, hardware.rs HW tab, mission.rs best-round restore, cto.rs, self_healing.rs (investigate), pipeline_api.rs (defer) | **Archive after Tier-1/2 ports land.** |
| **battle-command-forge** (top-3 ship candidate per review) | not at D:\dev\ — only at abcc_projects/ Archive/ | Reference | 5-in-1 critique call, `__init__.py` sanitization, content-aware fence extraction, venv-per-project verifier, surgical fix-pass discipline | **Keep as ship candidate** per user plan; don't archive until shipped. |
| **agent-battle-command-center (godfather)** | `D:\dev\agent-battle-command-center\` (intact) | Already public (25 stars). Solo. | CodeX-7 + Sentinel-9 verbatim agents, audio .wav banks (192 files), service patterns (humanEscalation, trainingData), 3s/8s rest timing constants | **Don't archive** — already public; just reference for lifts. |
| **battleclawclean** | `D:\dev\battleclawclean\` | v2 clean snapshot. Hadar-touched. | Patterns only (3-layer shell, validators) | **Archive.** |
| **battleclaw-forge-main** | `D:\dev\battleclaw-forge-main\` | Mar-8 MVP-readiness snapshot. Hadar-touched. | Double-Context Phase-0 gambit *pattern* | **Archive.** |
| **algo-trading-bot** | `D:\dev\algo-trading-bot\` | Solo Python | Niche — markets API knowledge only; defer | **Keep** if user still trades; archive otherwise. |
| **sui-arb-bot** | `D:\dev\sui-arb-bot\` | Solo Python | Niche; defer | Same as above. |
| **base-liquidator** | `D:\dev\base-liquidator\` | Solo Python | Niche; defer | Same. |
| **battle-claw** | `D:\dev\battle-claw\` | Skill definition only | SKILL.md as reference | **Archive.** |
| **landing_page_grok** | `D:\dev\landing_page_grok\` | Marketing asset | Reuse when claudette launches | **Keep.** |
| **life_hacker** | `D:\dev\life_hacker\` | **Empty directory.** | Nothing | **Archive.** |
| **publicators-monitor** | `D:\dev\publicators-monitor\` | One PS1 + one CSV | Nothing | **Archive.** |
| **stealthforge_tests** | `D:\dev\stealthforge_tests\` | Test overflow | Corpus only | **Archive.** |
| **abcc_projects/Archive/** | `D:\dev\abcc_projects\abcc_projects\Archive\` | History | All consumed by PROJECT_REVIEW.md §11 | **Keep** as historical reference. |

---

## 7. Suggested execution order

If the user agrees with the Tier-1 set, the natural sprint order minimizes risk:

1. **Persona system + Eva** (§2.1). One PR. Personas as MD files in `crates/claudette/personas/`, loader in `forge/personas.rs` (already exists), `--faceless` flag.
2. **Paste-to-tempfile + typewriter + Space Invaders** (§2.9, §2.10). One PR. TUI-only, low blast radius.
3. **Best-round restore + smart stopping** (§2.7). One PR in forge. Direct safety upgrade.
4. **Double-Context Phase-0 gambit** (§2.8). Small follow-up to forge fix-loop.
5. **5-tier permissions** (§2.2). Touches every tool call site — schedule deliberately, one PR with full test sweep.
6. **CTO chat agent** (§2.5). Builds on personas being live.
7. **SWE-bench runner** (§2.3) + **Multi-template bench + A/B** (§2.4) — bench crate. Two PRs, can land in either order.
8. **Antipattern auto-detection** (§2.6) — closes the feedback loop. Land after bench is live (so we have real failure data to test against).

Tier-2 items (LanceDB rich memory, Independence reviewer, voice banks, etc.) are decision-points, not ports — gather data from §1-§7 first.

---

## 8. What "claudette only" means after this sweep

Per the user's directive ("after this massive sweep we will focus only on claudette"):

- **claudette** stays the live product line.
- **claudettes-forge** (the parallel rewrite scaffold) becomes either: (a) folded back into claudette by lifting its scaffolded modules and abandoning the standalone repo, or (b) kept frozen as the "v2 architecture sketch" but no new work.
- **clawForge / life-hacker / battleclawclean / battleclaw-forge-main / battle-claw** → archive after lifts.
- **battle-command-forge / ABCC godfather** → keep around as already-public ship candidates per §14 — but the user said "focus only on claudette" so these likely freeze too unless re-launched.
- **Trading bots** → user's call; orthogonal to coding-agent work.

Recommend the user pick: **(a) fold claudettes-forge into claudette now**, or **(b) leave claudettes-forge frozen and absorb just the §2 Tier-1 modules into claudette**. Either way, the result is "claudette only."

---

## 9. Cross-reference to memory

This sweep extends:
- `project_audience_expansion_2026_05_15.md` — installer/show-me/PRIVACY already shipped.
- `project_e2e_sweep_2026_05_16_round3.md` — all 9 surfaces GREEN; this sweep targets *new* features, not gaps in shipped ones.
- `project_tui_sweep_2026_05_17.md` — §7 safety GREEN; §8 compaction + §9 edges still pending and are higher priority than the Tier-1 imports here.
- `project_forge_mode_shipped.md` — forge v0a/b/c live; §2.5 CTO + §2.6 antipattern + §2.7 best-round restore are the natural next forge upgrades.

The single largest insight: **PROJECT_REVIEW.md (Apr 2026) already triaged the abcc_projects/ tree; this sweep only adds what that doc didn't see** (the D:\dev top-level repos + claudette's post-Apr evolution).
