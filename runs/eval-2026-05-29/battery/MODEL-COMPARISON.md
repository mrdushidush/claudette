# Claudette local-model comparison — 50-task daily-driver battery

**Goal:** measure how well different local models drive claudette as a coding agent,
on the *same* objective 50-task battery, so we can recommend models to users and
show the project is rigorously benchmarked.

**Harness:** claudette **v0.8.1** (the cargo-installed binary on PATH — see WDAC note
below), daily-driver config (`CLAUDETTE_AUTO_APPROVE=1`, per-task
`CLAUDETTE_WORKSPACE`, OpenAI-compat → LM Studio @ `localhost:1234`). Every task runs
the real one-shot tool loop against a fresh fixture copy, then an **objective**
verifier (build/test passes, file exists with correct behavior, or transcript
contains ground-truth tokens). No self-grading.

**Hardware:** RTX 5060 Ti 16 GB (models > 16 GB spill to system RAM).

**LM Studio load settings (held constant):** **context = 24 576 (24k)** and
**`--parallel 1`** for every model. `--parallel 1` is essential — with N>1,
llama.cpp/LM Studio splits the context window into N slots (parallel 4 @ 24k ≈ 6k
usable per request), which silently starves the long-context bigrepo tasks. All
models are loaded identically for a fair comparison.

**Coverage:** 11 languages/surfaces (Rust, Python, JS, TS, Go, shell, HTML, CSS,
SQL, a large real-repo = claudette's own `src`+`docs`, git) × 12 task types
(bugfix, add-feature, multi-file, refactor, create-file, explain, locate,
enumerate, run-tests, debug-error, git-workflow, answer-from-codebase).

**Reasoning capture:** each model's run is recorded with
`lms log stream --source model --stats` → `~/claudette-eval-captures/<id>.stream.log`,
so we can inspect *which models reason correctly* vs. flail, not just the score.

Reproduce one model:
```
bash runs/eval-2026-05-29/battery/run_model_eval.sh <model-key> <identifier> 24576
```

---

## Lineup (from `lms ls`)

| # | Model (identifier) | LM Studio key | Params | Arch | Size | Status |
|---|---|---|---|---|---|---|
| 0 | q3-35b *(reference)* | `qwen3.6-35b-a3b@q3_k_xl` | 35B-A3B MoE | qwen35moe | 18.6 GB | baseline @24k p1 |
| 1 | q4-35b | `qwen3.6-35b-a3b@q4_k_xl` | 35B-A3B MoE | qwen35moe | 24.2 GB | pending |
| 2 | qwen3.6-27b (dense) | `qwen3.6-27b` | 27B dense | qwen35 | 14.2 GB | pending |
| 3 | qwen3.5-9b | `qwen3.5-9b` | 9B dense | qwen35 | 11.4 GB | pending |
| 4 | qwen3.5-4b | `qwen3.5-4b` | 4B dense | qwen35 | 7.3 GB | pending |
| — | ~~coder-30b~~ | `qwen3-coder-30b-a3b-instruct` | 30B-A3B MoE | qwen3moe | 17.7 GB | **SKIPPED** (broken template) |
| 5 | gpt-oss-20b | `openai/gpt-oss-20b` | 20B | gpt-oss | 12.1 GB | pending |
| 6 | nemotron-omni-reasoning | `nvidia-nemotron-3-nano-omni-30b-a3b-reasoning` | 30B-A3B MoE | nemotron_h_moe | 24.9 GB | pending |
| 7 | gemma-4-26b | `gemma-4-26b-a4b-it` | 26B-A4B | gemma4 | 19.2 GB | pending (crash risk) |

---

## Results

| Model | ctx | parallel | PASS/50 | % | total wall | slowest task | notes |
|---|---|---|---|---|---|---|---|
| q3-35b *(ref, 2026-05-29)* | 32k | 1 | 44/50 | **88.0%** | ~41 min | I3 240s (timeout) | the daily-driver baseline |
| q3-35b *(re-baseline)* | 24k | 1 | 46/50 | **92.0%** | 37.9 min | I3 191s (FAIL) | ✓ ≥88% ref — 24k holds. Crisp, correct diagnoses; follows repo_map→grep→read→run_tests workflow; verifies via tests. Fails: A4 incomplete refactor (missed a call site — under-enumeration, not bad logic), I1/I3 bigrepo enumerate+deep-locate weak spots, C4 correct-but-timed-out (EC=124) |
| q4-35b | 24k | 1 | 44/50 | **88.0%** | 48.3 min | I3 240s (timeout) | Reasoning quality ≈ q3, but **24.2 GB spills to RAM on the 16 GB GPU → ~20% slower → timeouts**. create-file only 1/4: B4 artifacts-correct-but-timed-out, C4+F2 didn't finish in time. Real miss: A5 (applied edit, skipped `cargo test` verify). I1/I3 bigrepo weak spots persist. **Takeaway: higher precision doesn't help here — q3 (fits VRAM, faster) completes more within the timeout.** |
| qwen3.6-27b (dense) | 24k | 1 | *32/37* **(86% of scored, stopped at 37/50)** | — | ~67 s/task | A4 151s | **Precision tier, not interactive.** Re-run to near-completion after the thermal concern was walked back (inference airflow keeps the coupled NVMe cool — only idle-GPU+download spikes it). **Accurate on what it finishes — bugfix 6/6, refactor 4/4, multi-file 4/4, rust 7/7, locate 2/2** — but **dense → ~67 s/task** (4–8× the A3B MoE) and **create-file 1/4**: loses generation-heavy tasks to the 150s wall (B4/C4/H2 timeouts, F2 correct-but-timed-out) + one real miss (C7). Highest SWE-bench in the Qwen line per scouts (77.2). **Use where correctness-per-attempt > speed (one-shot hard problems, batch jobs); not an interactive daily driver.** |
| qwen3.5-9b | 24k | 1 | 44/50 | **88.0%** | **15.7 min** | I3 108s | **Fastest by far — 2.4× q3, ZERO timeouts**, fits 11.4 GB VRAM, runs cool (GPU ~60°C). Excellent for a 9B. Failure mode is the inverse of the big models — *no throughput misses, pure correctness misses* on the hardest tasks: enumerate 1/3 (under-counts — I6 0/7 CLI modes, I1 2/6 vars), + two real logic errors (E1 left the buggy line; H4 wrong SQL column). **Best "fast / low-VRAM" pick.** |
| qwen3.5-4b | 24k | 1 | 45/50 | **90.0%** | **7.9 min** | C4 22s | **Astonishing for a 4B / 7.3 GB** — beats the 9b & q4, nearly matches the q3 champ, at **4.8× q3 speed**, ZERO timeouts, **create-file 4/4** (fast enough to finish what bigger models time out on). Fits **8 GB VRAM**. Real misses only on the hardest tasks: C1 (js bugfix), D4 (ts multi-file — missed a field), I1/I3/I8 (bigrepo enumerate/locate/answer). **The universal-access pick — runs on almost any GPU.** |
| gpt-oss-20b | 24k | 1 | 43/50 | **86.0%** | **5.3 min** | B4 29s | **Fastest run of the whole sweep** — MoE/MXFP4, fits **16 GB RESIDENT**, coolest load. **Harmony tool-calling works fine via LM Studio** (the scout caveat only bites raw llama.cpp). ZERO timeouts. Weakest on the long-context bigrepo set (4/8: I1/I3/I6/I7) + a SQL-dialect slip (H3: MySQL `AUTO_INCREMENT` in SQLite). Solid on standard single-file tasks (py 7/7, rust 7/7). **Best speed/efficiency pick.** |
| nemotron-omni-r (MoE, reasoning) | 24k | 1 | **n/a — too slow** | — | ~73 s/task | A2 145s | **Reasons correctly (3/3 PASS at stop) but impractical at 24k on 16 GB.** 24.9 GB spills to RAM → prompt-processing dominates each turn, and reasoning thinking-blocks compound it → **~73 s/task avg** (A2 hit 145s, a hair under the 150s wall) vs 9–30 s for the fast models. Projected ~60–80 min with heavy timeouts on the long-context tasks. Stopped at 3/50 after re-running with the user's adjusted load config (still too slow). MoE, so the GPU stays cool — the bottleneck is **RAM-spill + reasoning latency, not heat**. **Verdict: needs a smaller quant or lower context to be a viable daily driver here.** |
| gemma-4-26b (MoE) | 24k | 1 | **TEMPLATE-BLOCKED** | — | — | — | **Loaded fine** (19.2 GB, 24k/p1) but **stock GGUF chat template can't render tool requests in LM Studio** → HTTP 400 `"Cannot call something that is not a function: got UndefinedValue"`. Smoke-gated out before the full run (same class as coder-30b, different jinja bug). **Not a quality result.** Fix = `lmstudio-community` GGUF (LM Studio's own error message suggests this). MoE, so would run cool if templated correctly. |
| granite-4.1-8b (dense) | 24k | 1 | 39/50 | **78.0%** | 17.2 min | B2 161s | **Tool-calls reliably** (clean native templates — the scout's selling point holds: git/multi-file/refactor all 4/4) but a **weaker coder**: bugfix 2/6, debug-error 2/4, sql 1/3. Misses are genuine code-quality errors (A1 over-eager signature change, D1 duplicate decl, F1 shell syntax, H4 wrong column), not tool failures. 9.35 GB, fast + cool. **Good for tool-heavy/agentic orchestration; below the qwen small models for raw bugfixing.** |
| glm-4.7-flash (MoE) | 24k | 1 | **n/a — tool calls not rendering** | — | — | — | Loaded fine (13.78 GB resident, MoE/cool) but **narrates fixes in prose instead of emitting tool calls** → smoke A1 FAIL 102s (claudette loop-breaker fired). Tool schemas *were* sent (581 refs in capture), so it's a rendering/template issue: needs LM Studio runtime update + a post-21-Jan-2026 quant + corrected/`--jinja` template (sigmoid `scoring_func` fix). **Config issue, not a quality result** — to re-bench after fix. Scouts rate it highly (SWE 59.2, τ²-bench 79.5) when templated right. |

---

## Findings

### Windows Application Control (WDAC) blocks the freshly-built binary
On this machine, the locally-compiled `target/release/claudette.exe` is blocked by
**Windows Application Control** ("An Application Control policy has blocked this
file"), which pops a per-launch dialog and makes unattended runs fail in 0 s with
`exit 126` (permission denied). The **cargo-installed `~/.cargo/bin/claudette.exe`**
(same v0.8.1) is already approved and runs fine, so the harness defaults to the PATH
binary (override via `CLAUDETTE_BIN`). It deliberately does **not** probe
`target/release` (the probe itself triggers the popup). This is an environment quirk,
not a claudette issue — but it's exactly what an end-user compiling from source on a
locked-down Windows box would hit, so worth a docs note.

### Chat-template compatibility is a hard gate (independent of model quality)
**Two of the eight models never ran a single task** because their stock GGUF chat
templates can't render tool schemas in LM Studio's (C++ minja) engine — HTTP 400
before the model is even invoked:
- **`qwen3-coder-30b-a3b-instruct`** → `Unknown StringValue filter: safe` (a Jinja
  `| safe` filter minja doesn't implement). *(pre-skipped)*
- **`gemma-4-26b-a4b-it`** → `Cannot call something that is not a function: got
  UndefinedValue`. Loaded fine (19.2 GB, 24k/p1) — purely a template-render failure.
Neither is a claudette bug or a model-quality result. **Both are fixable** via the
`lmstudio-community` re-published GGUF (fixed templates) or a 30-second template
override; LM Studio's own 400 message even points you to lmstudio-community. **This is
the #1 local-model failure mode** — always pull `lmstudio-community → unsloth →
bartowski` GGUFs and validate one real tool-call round-trip before trusting a model.
(See `CANDIDATES.md` for the full template playbook + the froggeric rescue templates.)

### `--parallel 1` matters
Loading at the LM Studio default (`--parallel 4`) divides the 24k context into ~6k
per slot, starving the long-context tasks. Always load with `--parallel 1` for
single-agent use.

### Heat is driven by dense-vs-MoE, NOT model size
Sustained GPU temperature tracks **active parameters**, not file size: the A3B/A4B
**MoE** models (q3/q4-35b-a3b, gpt-oss-20b, gemma-4-26b-a4b) run the GPU cool (~55 °C)
even at 19–25 GB, while the **dense** qwen3.6-27b pins it at 96 %/72 °C — which is why
that one run was halted for thermal management. Practical rule: prefer MoE for
sustained local use; a big MoE is cooler than a mid-size dense model.

### VRAM/offload at 24k decides throughput — and throughput decides the score
On a 16 GB card, models >16 GB spill to system RAM at 24k and run ~20 % slower. That
slowdown, not reasoning, is what cost the bigger models points: **q4-35b (24 GB) lost
4 tasks purely to create-file/`generate_code` timeouts** that the same-family q3 (fits
VRAM) finished. The single best predictor of "fits in the 150 s task budget" was **does
it fit in VRAM**, not parameter count. gpt-oss-20b (MXFP4, ~13 GB, fully resident) was
the fastest of all at 5.3 min.

### Two distinct failure modes — and small models flip which one dominates
- **Big models fail by timeout** (correct work that doesn't finish): q4 B4/C4/F2,
  qwen3.6-27b B4/C4 — all create-file/generate_code that timed out, artifacts often
  correct.
- **Small models fail by correctness** (fast but wrong): qwen3.5-9b/4b and gpt-oss-20b
  produced *zero* timeouts but made outright errors on the hardest tasks (E1 left a bug
  in, H4/H3 SQL slips, D4 missed a field).
- **Model-bound weak spots persist across the whole lineup:** the bigrepo
  **enumerate** (I1/I6 — under-counting) and **deep-locate-with-conflicting-docs** (I3 —
  trusting stale docs over source) tasks failed for nearly every model. These are the
  real frontier, independent of size/speed.

### The surprise: small models are the value play
A 4B model (qwen3.5-4b, 7.3 GB) scored **90 % in 7.9 min** — beating the 9B and the
q4-35b, nearly matching the q3-35b champ (92 %), and finishing **4.8× faster** with
zero timeouts. For an air-gapped assistant meant to run on *anyone's* hardware, the
fast small models (qwen3.5-4b/9b, gpt-oss-20b) are the headline: ~86–90 % at a fraction
of the VRAM and wall-time of the 35B.

### Recommendation tiers (for the README)
- **Best accuracy:** `qwen3.6-35b-a3b @ q3_k_xl` — 92 %, fits 16 GB, MoE/cool; the
  daily-driver default. (q4_k_xl spills to RAM and scores *lower* via timeouts — skip it.)
- **Best all-round / value:** `qwen3.5-4b` — 90 % at 8 GB and 8 min; runs on almost
  anything.
- **Fastest / lowest overhead:** `gpt-oss-20b` — 86 %, fully VRAM-resident at ~13 GB,
  5 min, coolest.
- **Solid mid:** `qwen3.5-9b` — 88 %, 11 GB.
- **Avoid for now:** q4_k_xl (RAM-bound), dense qwen3.6-27b (hot/slow), and any
  stock-template model that 400s (gemma-4, coder-30b) until repacked.
- **Pending:** nemotron-omni-r (reasoning MoE) — to be re-run after reconfiguration.
