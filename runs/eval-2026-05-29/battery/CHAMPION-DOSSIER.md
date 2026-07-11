# CHAMPION DOSSIER — qwen3.6-35b-a3b deep-tune (campaign start 2026-07-11)

Single-model campaign: max quality AND speed for **qwen3.6-35b-a3b** on the RTX 5060 Ti
16 GB / Windows 11 box. Goal doc: `launch-drafts/goal_champion_tuning_2026_07_11.md`.

## Baseline (FREE row — reuse, don't re-run)

| config | PASS/50 | K/8 | wall | notes |
|---|---|---|---|---|
| `champ-q3kxl-lms` = unsloth UD-Q3_K_XL 16.8 GB @ LM Studio, ctx 24576, par 1 | **47/50 (94%)** | **8/8** | ~32 min | sole miss I8 (timeout); v0.16.0 sweep 2026-07-10 |

**Rig + versions (2026-07-11):** RTX 5060 Ti 16 GB (16311 MiB), driver **610.62**, GPU idle 53 °C ·
LM Studio app **0.4.19** (CLI commit 9902c3a) · selected runtime **llama.cpp-win-x86_64-nvidia-cuda12-avx2@2.24.0** ·
claudette **0.16.0** (`~/.cargo/bin`, main `079f69a`) · May MTP server build: `D:\dev\llama.cpp-mtp`
(am17an mtp-clean, HEAD `2dff7ff`, CUDA 13.2.78, SM_120a) · disk free: C 1315 GB / D 977 GB.

---

## 1. Quant census

### unsloth `Qwen3.6-35B-A3B-GGUF` (NTP — no MTP head) — incumbent lineage

Full UD ladder (single-file). The 16 GB-relevant band:

| quant | size | note |
|---|---:|---|
| UD-Q2_K_XL | 12.3 GB | floor probe (optional SCREEN-10) |
| UD-IQ3_XXS / IQ3_S | 13.2 / 13.7 GB | not planned — Q3_K_XL already proven |
| UD-Q3_K_S / Q3_K_M | 15.4 / 16.6 GB | skipped — dominated by Q3_K_XL |
| **UD-Q3_K_XL** | **16.8 GB** | **incumbent (47/50)** |
| **UD-IQ4_XS** | **17.7 GB** | ladder #1 — cheapest 4-bit jump |
| UD-IQ4_NL / IQ4_NL_XL | 18.0 / 19.5 GB | alt 4-bit; only if IQ4_XS disappoints |
| **UD-Q4_K_S** | **20.9 GB** | ladder #3 (fallback) |
| UD-Q4_K_M | 22.1 GB | dominated by Q4_K_XL (+0.3 GB) |
| **UD-Q4_K_XL** | **22.4 GB** | ladder #2 — max-quality tier; **ON DISK** |
| MXFP4_MOE | 21.7 GB | **measured null/negative 2026-05-16** — skip |
| Q5+ tiers | 24.9–38.5 GB | out of reach (32 GB system RAM; ~9+ GB spill already at Q4_K_XL) |

### unsloth `Qwen3.6-35B-A3B-MTP-GGUF` (same UD lineage + MTP head, ~+0.4–0.6 GB/file)

Full mirror ladder. Relevant: UD-Q3_K_XL 17.2 · **UD-IQ4_XS 18.2** · UD-Q4_K_S 21.4 ·
**UD-Q4_K_XL 22.9 (ON DISK at `C:\models\`, benchmarked 2026-05-16)** · MXFP4_MOE 22.2 (null).
Ships mmproj (vision) files — not needed for battery. MTP head bundled in-file, no `-md` draft.

### byteshape `Qwen3.6-35B-A3B-MTP-GGUF` (ShapeLearn — DIFFERENT quantizer lineage)

Per-tensor learned datatypes; "GPU-N" labels ≈ nearest llama.cpp profile. Their **own RTX 5060 Ti
plot** (blog "If It Fits, It Sits", 2026-05-19):

| variant | bpw | size | 5060 Ti tok/s | quality (their acc vs bf16) |
|---|---:|---:|---:|---:|
| GPU-1 (NTP) | 2.17 | ~10 GB | 132.1 | 0.887 |
| GPU-2 (NTP) | 3.00 | ~13 GB | 120.7 | 0.960 |
| GPU-3 (NTP) | 3.48 | 15.7 GB | 115.6 | 0.966 |
| GPU-4/5 (NTP) | 3.97/4.19 | 17.6/18.6 GB | — | no 5060 Ti data: **don't fit 16 GB** w/ reasonable ctx |
| **MTP-GPU-1** | 2.25 | 10 GB | **169.8** | 0.887 |
| **MTP-GPU-2** | 3.06 | **13.6 GB** | **169.5** | 0.960 |

byteshape's 16 GB recommendation: **MTP-GPU-2** (MTP memory footprint rules out higher tiers).
Quality benched on BFCL-V3 / LiveCodeBench / HumanEval / GSM8K / IFEVAL — NOT our battery;
lineage ≠ unsloth, so **battery decides**.

> ⚠ Goal-doc deviation (allowed under "better ideas"): goal suggested byteshape 3.97/4.19 bpw
> first. byteshape's own 5060 Ti data shows those tiers don't fit; swapping the byteshape
> candidate to **MTP-GPU-2 (13.6 GB, fully resident, 169 tok/s claimed)** — the only lineage +
> size that promises **VRAM residency**, which their data says is worth 2.5–4× vs our spilled
> Q4 numbers.

### Others
- **lmstudio-community**: Q4_K_M 21 GB on disk (+ BF16 mmproj). Same-ish tier as unsloth Q4_K_M; nothing unique.
- **Official Qwen GGUFs**: 22 GB q4 **timed out under mem pressure in the 2026-07-10 sweep** (screener `SCORES-screen-q36-official-v0160.tsv`); dominated by unsloth UD.
- **bartowski**: standard imatrix ladder, no MTP variants; nothing the unsloth ladder doesn't cover.

## 2. Server census

| server | MTP | verdict for this campaign |
|---|---|---|
| **LM Studio 0.4.19** (installed) + runtime 2.24.0 | **YES — native since 0.4.14 (2026-05-22)**: load-time toggle for MTP-GGUF models; KV-quant + FA per-model in Advanced UI | **Front-runner.** Fewest moving parts; 2.24.0 is the exact runtime the 47/50 baseline used. MTP rows can run INSIDE LMS — the goal's Phase 4.5 contingency is live |
| **llama.cpp `llama-server`** master | MTP **merged upstream 2026-05-16** (PR #22673, am17an — same code we built) | Our May build (`2dff7ff`) ≈ master-at-merge; proven 45.7 tok/s w/ `--fit-target 2304`. Official Windows CUDA prebuilts exist (CUDA 13.1+) but SM_120 not in default arch list + community reports CUDA-13 MMQ regressions on Blackwell (Mar 2026: "CUDA 12.8 + MMQ optimal"). **Keep the proven source build**; only rebuild master if a specific perf PR justifies it. Unique value vs LMS: `--fit-target` expert-packing (LMS lacks it) |
| ik_llama.cpp | fork lags upstream MTP | Windows-CUDA buildable but MoE gains centre on CPU/hybrid paths; **skip unless llama-server leaves speed on the table** (goal gate) |
| ollama | no MTP, no fit-target, no KV-quant flags | **skip** — claudette already speaks OpenAI-compat to any server; ollama adds a wrapper and removes knobs |

Claudette repoint (verified `crates/claudette/src/api.rs:204-296`): `OLLAMA_HOST=http://localhost:<port>` +
`CLAUDETTE_OPENAI_COMPAT=1`. Battery harness already exports exactly these (`run_battery.sh:22-23`).

## 3. Speed-knob census (× = measured on THIS box, 2026-05-16 archive `docs/archive/mtp_benchmark.md`)

| knob | effect (measured ×/claimed) | note |
|---|---|---|
| × MTP `--spec-type draft-mtp` | **1.77×** gen (24.95→43–45.7 tok/s) on spilled Q4_K_XL | acceptance 84% synthetic / **88% under real forge load**; pre-05-13 alias `mtp` silently no-ops |
| × `--spec-draft-n-max` | **2 = peak** (1→43.7, 2→45.7, 3→45.3, 4→42.6, 6→36.9) | sweep DONE in May — reuse, spot-check on new quant only |
| × `--fit-target` | **2304 = peak** (2048 over-packs −11%, 3584 −6%; default 1024 margin SPILLS → 0.42×!) | llama-server only; retune if quant/ctx changes |
| × `--no-mmap` | **+9.4% tok/s, +10.2 GB free RAM** | LMS has the toggle in per-model Configure panel too |
| × `--cpu-moe` | 1.16× only — **worse than fit-target packing** | don't use |
| × MXFP4 | null/negative on BOTH backends (Blackwell FP4 doesn't pay; bandwidth-bound) | closed question |
| × `--cache-ram` | 1024 (default) ≥ 8192 | keep default |
| KV q8_0 | ~halves KV; in prod config since May, quality-neutral there | q4_0 KV = quality risk, only try if 64k doesn't fit |
| FA | ON everywhere since May | table stakes |
| **VRAM residency** | byteshape 5060 Ti: resident 3-bit tiers run **115–170 tok/s** vs our spilled-Q4 45.7 | **the biggest single lever on this card** — quality-vs-residency is the campaign's real tradeoff |

## 4. LoRA landscape (feeds Phase 5 feasibility report — research only)

- **Local training: NO.** unsloth: MoE **QLoRA not recommended** for 35B-A3B; bf16 LoRA ≈ **74 GB VRAM**
  (H100-80G class). 16 GB box is out by ~4.6×.
- **Seq-length ceiling: 2048** on a single 80 GB card (backward pass at 4096 OOMs). Claudette
  transcripts are agentic multi-turn (8k–32k) → needs truncation/packing tricks or 2× GPUs —
  the *quiet killer* of the naive behavioral-cloning plan.
- **MoE LoRA best practice**: freeze router (destabilizes training; pretrained routing generalizes),
  LoRA on attention (+ optionally top-routed experts per MoE-Sieve); unsloth trains 30B-A3B in
  17.5 GB *for the smaller model* via 4-bit on-the-fly — not offered for 35B.
- **Cloud $**: RunPod on-demand A100-80G $1.39–1.49/h, H100 $2.89/h (community/spot: Vast A100
  ~$0.67–0.79/h). Realistic attention-only LoRA SFT (2–5k examples, 1–2 epochs, seq-capped):
  ~10–30 A100-hours ≈ **$15–50 compute**; double for eval loops/restarts.
- **MTP-head risk (novel, important)**: LoRA targets attention/experts → merge leaves MTP head
  weights *untouched* — GGUF re-conversion keeps the head (PR #22673 conversion path). BUT the
  head then predicts the OLD policy's next tokens → **draft acceptance (84–88%) degrades → the
  1.77× speedup partially evaporates**. Mitigation: include MTP head in trainable params (unsloth
  support unclear) or accept slower drafts.
- **Data assets on hand**: successful battery transcripts (logs-* dirs), `~/claudette-eval-captures/
  *.stream.log` reasoning captures, dogfood transcripts. Target weak spots: I3/I5 deep-locate, I8,
  J-git under weaker quants.
- **Overfit risk**: core-50 is tiny; training on battery-adjacent data invalidates the battery as
  the eval. Would need a held-out split + fresh tasks — collides with frozen-core-50 discipline.

## 5. Campaign test matrix (live — updated per config)

Tag scheme `champ-<quant>-<server>[-knobs]`. All battery rows @ ctx 24576, `--parallel 1`,
KV q8_0 + FA ON (recorded per row). Speed probe = `probe_speed.sh` 3-prompt median.

| # | tag | quant (GB) | server | battery | speed probe | status |
|---|---|---|---|---|---|---|
| 0 | `champ-q3kxl-lms` | UD-Q3_K_XL 16.8 | LMS | 47/50 + K8/8 (reused) | probe pending | **BASELINE** |
| 1 | `champ-iq4xs-lms` | UD-IQ4_XS 17.7 | LMS | full + K | yes | pending download |
| 2 | `champ-q4kxl-lms` | UD-Q4_K_XL 22.4 | LMS | full + K | yes | on disk |
| 3 | `champ-q4ks-lms` | UD-Q4_K_S 20.9 | LMS | only if #2 thrashes | yes | contingent |
| 4 | `champ-mtp-q4kxl-lms` | MTP UD-Q4_K_XL 22.9 | LMS+MTP toggle | SCREEN-10 + K (lineage = unsloth, base quant battery-cleared via #2) | yes | on disk (C:\models — needs LMS import) |
| 5 | `champ-mtp-<win>-lms` | MTP twin of Phase-3 winner | LMS+MTP | SCREEN-10 + K | yes | contingent |
| 6 | `champ-bs-mtpgpu2-lms` | byteshape MTP-GPU-2 13.6 | LMS+MTP | **FULL + K** (new lineage) | yes | pending download |
| 7 | `champ-mtp-q4kxl-llsrv` | MTP UD-Q4_K_XL 22.9 | llama-server `2dff7ff` fit-2304 | SCREEN-10 + K parity | yes (have May #s) | build ready |
| 8 | `champ-q2kxl-lms` | UD-Q2_K_XL 12.3 | LMS | SCREEN-10 only (floor probe) | optional | time-permitting |

Decision cols (final matrix in §6 when rows land): PASS/50 · K/8 · wall · gen tok/s · spill GB ·
moving parts · template health.

## 6. Decision matrix — TBD (filled at crown time)

## 7. Results log (checkpoint per config)

- 2026-07-11: campaign start. Phase 0 ✓ (versions above). Phase 1 census ✓ (this doc).
- 2026-07-11: harness v2.1 shipped (`BATTERY_BASE_URL` + `BATTERY_SKIP_LMS` in
  run_battery/run_model_eval/run_screener + `probe_speed.sh` → `SPEED-PROBES.tsv`).
- 2026-07-11 **`champ-q3kxl-lms` speed probe (baseline)**: **33.83 tok/s** median
  (ttft 2.24 s, VRAM 14,990 MiB, ctx 24576). Settings recovered from the per-model
  LMS config the 47/50 ran with: KV q8_0 K+V, **no-mmap, expert-CPU-ratio 0.4**,
  parallel 1, FA=runtime default (no explicit key). Config JSONs live at
  `~/.lmstudio/.internal/user-concrete-model-default-config/unsloth/Qwen3.6-35B-A3B-GGUF/`.

## Sources (Phase 1 census)

- unsloth NTP + MTP GGUF trees: huggingface.co/unsloth/Qwen3.6-35B-A3B-GGUF · /Qwen3.6-35B-A3B-MTP-GGUF
- byteshape: huggingface.co/byteshape/Qwen3.6-35B-A3B-MTP-GGUF · byteshape.com/blogs/Qwen3.6-35B-A3B/
- MTP merge: github.com/ggml-org/llama.cpp PR #22673 (merged 2026-05-16)
- LM Studio MTP: lmstudio.ai/changelog 0.4.14 (2026-05-22) · x.com/lmstudio status 2057889028578455905
- Blackwell/CUDA state: llama.cpp issue #22696 · zenn.dev toki_mwc Blackwell CUDA-toolkit trap
- ik_llama.cpp: github.com/ikawrakow/ik_llama.cpp (build.md)
- LoRA: unsloth.ai/docs/models/qwen3.5/fine-tune · /docs/basics/faster-moe · arxiv 2603.24044 (MoE-Sieve) ·
  ms-swift issue #5512 · runpod.io/pricing · computeprices.com/providers/runpod
- Our own May data: docs/archive/mtp_benchmark.md (2026-05-16)
