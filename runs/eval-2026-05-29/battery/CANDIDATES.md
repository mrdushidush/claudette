# Claudette-Certified — candidate models to battery-test (scouted 2026-05-30)

Sourced from 7 parallel web-research agents. Selection priorities: strong coding,
**reliable tool/function-calling**, **MoE preferred** (cooler GPU per
[thermal standard] — dense mid/large models run hot), GGUF with a **working LM Studio
chat template** (broken jinja `| safe` / tool-role templates silently kill the tool loop),
fits ~16 GB VRAM @ 24k ctx (also note other tiers). Recency favored (2025–2026).

> Status: collecting agent results. Synthesize + dedupe + rank when all 7 are in.

---

## Mistral family  *(agent 4 — done)*
**Caveat:** every Mistral model that fits 16 GB is **DENSE → thermally hot**; the MoE
Mistrals (Small 4 119B, Large 3 675B) are far too big for 16 GB. So the MoE/cool
preference can't be met in this family at this tier.

| Model | Arch | Fit @16GB/24k | Coding | Tool-calling | Template risk | Verdict |
|---|---|---|---|---|---|---|
| **Devstral Small 2** (24B-2512) | dense 24B | tight — IQ3 only (quant-sensitive; cliff <2.3bpw) | **68% SWE-bench** (best 24B agentic) | purpose-built code-agent (powers Vibe CLI) | ⚠ **stock GGUF template broken for tool-calls** (role-alternation errors; needs unsloth fix / pinned `--chat-template`, disable `--jinja`) | top capability, but dense+template risk; certify at IQ3 + fixed template |
| **Mistral Small 3.2** (24B-2506) | dense 24B | best fit — Q3_K_XL clean | all-rounder (no headline SWE) | **most proven/stable tool-caller** in family | early GGUFs buggy; unsloth/bartowski builds fixed | safest reliable default baseline |
| **Ministral 3 14B** (2512) | dense 14B | **comfortable Q4_K_M + 24k headroom** | mid (reasoning variant exists) | native FC + JSON | verify Mistral-3 tool-role template | best clean 16GB fit; pragmatic pick |
| Codestral 2 (22B, Apr 2026, Apache-2.0) | dense 22B | IQ3/Q3 | elite **FIM/completion** (~95% FIM, 86% HumanEval) | ⚠ **completion tool, NOT agentic** | n/a | mismatch — use as autocomplete sidecar, not the loop |
| Ministral 3 8B (2512) | dense 8B | trivial (~12GB) | modest (8B) | native FC + JSON | Mistral-3 template caveat | budget/router tier, fast smoke model |

**Disqualified for 16GB:** Mistral Small 4 (119B MoE, 6.5–22B active — needs ≥48GB; IQ2 = 31GB), Devstral 2 Large (123B dense, 72.2% SWE-bench).
**Universal Mistral gotcha:** Tekken-tokenizer + Jinja templates repeatedly ship broken tool-call round-trips in 3rd-party GGUF converts → always pull unsloth/bartowski/official tool-fixed builds and validate one full tool-call→result→assistant turn before certifying.
**Top Mistral picks for our battery:** Devstral Small 2 (max capability, dense/hot), Mistral Small 3.2 (safest), Ministral 3 14B (best fit). All dense → flag thermal.

---

## Recent MoE coders  *(agent 1 — done)*
> Note: 30B-total MoEs need llama.cpp expert-offload (`-ncmoe` / `--override-tensor experts=CPU`) to fit Q4+24k on 16 GB; only gpt-oss-20b is truly resident.

| Model | Arch (MoE) | Fit @16GB/24k | Coding | Tool-calling | Template | Verdict |
|---|---|---|---|---|---|---|
| **GLM-4.7-Flash** (Z.ai, Jan 2026) | 30B / ~3B act | Q3/Q4_K_XL + offload (Q4_K_M ~10GB per agent 6) | **SWE 59.2**, τ² 79.5 (multi-step tools) | tuned for agents (Cline/Goose/Zed) | ⚠ pre-21-Jan quants loop/break tools; **use post-21-Jan + `--jinja`** | freshest small-MoE agentic coder; top pick |
| **gpt-oss-20b** (OpenAI, Aug 2025) | 21B / 3.6B act, MXFP4 | **~12GB, RESIDENT on 16GB** (best fit, coolest) | LCB v6 **70** | native FC, TauBench-trained | ⚠ **Harmony format**; LM Studio has no jinja editor for it; use unsloth fixed GGUF, low/med effort | cleanest resident MoE — **testing now** |
| **Qwen3-Coder-30B-A3B** (Jul 2025) | 30B / 3.3B act | Q3_K_M 14.7 / Q4_K_XL 17.7 + offload | **SWE 51.6** | purpose-built agentic; XML tool format | ⚠ the canonical `\| safe` offender — **FIXED in post-Aug-2025 unsloth/v16 template** + `--jinja` | the model we skipped IS usable with a fixed GGUF — retry candidate |
| Qwen3.6-35B-A3B (Apr 2026) | 35B / 3B act | Q3/IQ + offload | vendor 73.4 SWE / repro ~53/100 | `qwen3_coder` parser | Qwen `\|safe` lineage; current GGUF + `--jinja` | our in-house default; repro SWE trails Qwen3-Coder-30B & 3.5 |
| Qwen3-Coder-Next 80B-A3B (2026) | 80B / 3B act | ❌ ~49.6GB — needs 48GB+ | **>70 SWE**, best per active-param | trained for tool use + error recovery | Qwen `\|safe` lineage | ceiling brain for 32GB+/workstation only |

**Excluded (too big for ≤32GB local):** GLM-4.5-Air 106B, DeepSeek V3.2/V4, Kimi-K2.x (1T), Ring 2.6 (1T), GLM-4.7 full 355B.

---

## Phi / Granite / Gemma  *(agent 5 — done)*
**Verdict: IBM Granite is the category winner** (only family purpose-built for tool calls w/ native llama.cpp-parsed templates). Phi-4 14B does NOT support function calling (only Phi-4-mini). Gemma 3/4 tool-calling is broken in stock GGUF runtimes.

| Model | Arch | Fit @16GB/24k | Coding | Tool-calling | Template | Verdict |
|---|---|---|---|---|---|---|
| **Granite 4.1-8B-Instruct** (IBM, Apr 2026) | dense 8B | **easy — Q4 ~5GB, huge headroom** | strong for 8B (30B sib HumanEval ~89) | **BFCL v3 68.3** (best in cat) | native tool tokens, clean LM Studio/Ollama | **category pick** — dense, Apache-2.0, reliable tools, low VRAM |
| Granite 4.0-H-Tiny (7B-A1B MoE) | hybrid Mamba MoE | roomy | lower (1B active) | "fast function calling" building block | ⚠ hybrid-Mamba needs recent llama.cpp build | best cool/low-VRAM MoE for high-volume simple loops |
| Granite 4.0-H-Small (32B-A9B MoE) | hybrid Mamba MoE | tight/marginal (~18-19GB) → 24GB tier | workhorse | BFCL 64.7 | hybrid-Mamba runtime caveat | true MoE for 24GB tier |
| Phi-4-mini-instruct (3.8B, Feb 2025) | dense 3.8B | trivial (~2.2GB Q4) | HumanEval 74 | only Phi w/ FC; **shaky w/ many tools** | `\|tool\|` JSON in system prompt; needs recent llama.cpp | tiny fallback, not primary |
| Gemma 4 26B-A4B (Google, Apr 2026) | MoE 26B / 3.8B act | marginal (26B wts) → 24GB | competitive general; not code-specialized | ⚠ **broken in stock Ollama/GGUF** (calls leak to content) | needs **patched llama.cpp** (PR #21326/#21343) | what we run — keep only on patched build; weak as pure agent brain |

**DQ:** Phi-4 14B (no FC, 16K ctx), no Phi-5 exists, Granite-Code superseded by 4.x.

---

## GLM / Llama / Cohere / Kimi / MiniMax / Nemotron  *(agent 6 — done)*
**Headline: GLM-4.7-Flash is the only model here that's both top-tier agentic AND fits 16GB.** (Corroborates agent 1.)

| Model | Arch | Fit @16GB/24k | Coding | Tool-calling | Verdict |
|---|---|---|---|---|---|
| **GLM-4.7-Flash** (Jan 2026) | 30B/3B MoE | **Q4_K_M ~10GB, comfortable** | **SWE 59.2**, LCB 84.9 | **τ² 79.5**, Claude-Code/Cline-validated | category + overall pick; post-21-Jan quant + `--jinja` + sigmoid-fixed build |
| Nemotron-Nano-12B-v2 (NVIDIA) | hybrid Mamba dense ~12B | easy Q4/Q5 | solid mid-tier | supported (vLLM parser proven) | lightweight non-MoE fallback; confirm `nemotron_h` runtime support |
| MiniMax-M2.7 | 230B/10B MoE | ❌ 24GB+ w/ offload | top agentic coder | native in LM Studio ≥0.3.31 | 24GB+ tier |
| GLM-4.5-Air | 106B/12B MoE | ❌ 24GB+ | ~59.8 | agentic | superseded by 4.7-Flash for 16GB |
| Llama 3.3 70B | dense 70B | ❌ 48GB+ | strong | mature JSON FC, best-supported | great brain if ≥48GB |

**DQ:** Kimi-K2.6 (1T, datacenter), Cohere Command-A 111B (thin GGUF), Seed-Coder-8B (SWE only 19.2, not agentic), Llama 4 Scout 109B (24GB+, weaker coder).

---

## Small dense ≤14B (fast/low-VRAM tier)  *(agent 2 — done)*
**Winner: the Qwen3.5 small dense series** (Mar 2026) — exactly our reference family.

| Model | Arch | Fit | Coding | Tool-calling | Verdict |
|---|---|---|---|---|---|
| **Qwen3.5-9B** | dense 9B | Q4_K_XL ~6GB (10-12GB comfy) | **LCB 82.7** | **BFCL-v4 66, τ² 79.1** | top pick — beats Qwen3-Next-80B on FC; **we ran it: 88%** |
| **Qwen3.5-4B** | dense 4B | Q4 2.7GB / Q8 4.3GB (8GB!) | IFEval ~90 | best 4B caller | fast/8GB pick — **we're running it now** |
| Ministral 3 8B (Dec 2025) | dense 8B | ~5GB Q4 | LCB ~62 | native FC+JSON | non-Qwen control |
| Phi-4 14B | dense 14B | ~9GB Q4 (tight) | HumanEval 83 | only mini has FC; shaky | reasoner not tool-caller |
| Granite 4.1/3.3 8B | dense 8B | ~5GB | mid | BFCL 68 | enterprise-clean fallback |
> **Critical for whole Qwen3.5/3.6 gen:** stock GGUF templates broken under LM Studio (`\|safe`/`\|items`/"Unknown test: sequence") — **use `lmstudio-community` repack or froggeric patched template; needs LM Studio ≥0.4.7.**

## Qwen + DeepSeek ecosystems  *(agent 3 — done)*
**Bottom line: our incumbent Qwen3.6-35B-A3B is still the strongest 16GB-class brain; new releases sit beside it, not above.**
| Model | Arch | Fit @16GB | Coding | Notes |
|---|---|---|---|---|
| Qwen3.6-35B-A3B (incumbent) | 35B/3B MoE | Q3 13-16GB | **SWE 73.4, LCB 80.4** | keep as primary; thinking-mode ON by default (set `enable_thinking:false`) |
| **Qwen3-Coder-30B-A3B** | 30B/3.3B MoE | Q3_K_M ~14.7GB | SWE 51.6 | **`\|safe` bug FIXED post-Aug-2025** unsloth/lmstudio-community + `--jinja`; pilot as coding specialist. ⚠ also a missing-`properties` 500-crash — ensure tool schemas emit `properties:{}` |
| DeepSeek-R1-0528-Qwen3-8B | dense 8B | ~5GB (trivial) | reasoning SOTA-8B | **BFCL 93** caller; `<think>` overhead; low-VRAM fallback |
| Qwen3.6-27B (dense) | dense 27B | Q3_K_M ~13.6GB | **SWE 77.2** | hotter (dense); we partial-tested it |
| DeepSeek-Coder-V2-Lite | 16B/2.4B MoE | easy | HumanEval 90 | cool but weak tool format; Chinese-drift; flash-attn OFF |
> Ruled out (too big): Qwen3-Coder-Next 80B (~21GB+), DeepSeek-V3.2/V4 (datacenter), QwQ-32B (loops — superseded).

## Tool-calling reliability + template playbook  *(agent 7 — done; the make-or-break axis)*
**Real-world multi-tool eval (jdhodges 2026, 13 models):** Qwen3.5-4B **97.5%** (best!), GLM-4.7-Flash 95%, Nemotron-Nano-4B 95%, Mistral-Nemo-12B 92.5%. **BFCL-v3 leader: GLM-4.5 ~77%** (note: old v1/v2 90%+ scores are NOT comparable to v3 multi-turn).
- **The `\|safe` fix is a 30-second LM Studio edit:** Prompt Template editor → delete ` \| safe` (2 spots in Qwen3-Coder). Or just pull a fixed publisher.
- **Publisher order for tool templates: `lmstudio-community` → `unsloth` → `bartowski` → froggeric-rescue.** Avoid stock Mistral/Step/KAT GGUFs for tools (no native jinja / broken). xLAM & Hammer ship their own tool-ready GGUFs.
- **llama.cpp escape hatch:** `--jinja --chat-template-file <fixed.jinja>`. **Keep quant ≥Q5 for small dense FC models** (JSON-arg fidelity degrades faster than prose; don't push MoE below ~IQ3).
- Specialized tool-routers: Salesforce **xLAM-2-3b/8b-fc-r**, MadeAgents **Hammer2.1-7b** (vendor GGUFs, narrow schemas).

---

# 🏁 CERTIFICATION QUEUE — next models to run on the 50-task battery
All confirmed to fit 16 GB @ 24k with a known working-template path. Priority order:

| Pri | Model | Why | Template action |
|---|---|---|---|
| 1 | **gpt-oss-20b** | MoE, only fully-resident 16GB fit, coolest, LCB 70 | use `lms get openai/gpt-oss-20b`; verify Harmony parse — **running now** |
| 2 | **GLM-4.7-Flash** | strongest new agentic coder, MoE/cool ~10GB, SWE 59.2/τ² 79.5, 95% real-world | post-21-Jan quant + `--jinja` + sigmoid-fixed build |
| 3 | **Qwen3-Coder-30B-A3B** | resolves our earlier SKIP — coding specialist, MoE | lmstudio-community GGUF (or delete `\|safe`) + `--jinja`; emit `properties:{}` |
| 4 | **Granite 4.1-8B** | best small dense tool-caller, BFCL 68, clean templates, tiny VRAM | native — low risk |
| 5 | **Nemotron-Nano-12B-v2** | reliable hybrid-dense, 95% real-world FC | confirm `nemotron_h` runtime support |
| 6 | **Ministral 3 8B** or **Mistral-Nemo-12B** | non-Qwen control; Nemo = best multi-turn sequencer | **unsloth** GGUF only (never stock Mistral) |
| 7 | **DeepSeek-R1-0528-Qwen3-8B** | low-VRAM reasoning fallback, BFCL 93 | unsloth post-Jun-2025 + `--jinja` |

**Already battery-tested this sweep:** qwen3.6-35b-a3b q3 **92%** / q4 88%, qwen3.6-27b dense (partial, thermal), qwen3.5-9b **88%**, qwen3.5-4b (in progress). Big-tier (24GB+) watch list: MiniMax-M2.7, GLM-4.5-Air, Qwen3-Coder-Next 80B, Llama 3.3 70B, Kimi-K2.6.

**MoE/thermal note** (per thermal standard): GLM-4.7-Flash, gpt-oss-20b, Qwen3-Coder-30B, Granite-4.0-H, DeepSeek-Coder-V2-Lite are MoE → cool. Devstral/Mistral-Small/Ministral/Qwen3.6-27B are dense → run hotter; Granite-4.1-8B & Nemotron-12B are small-dense (mild).

