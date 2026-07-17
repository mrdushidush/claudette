# Hardware requirements

## Baseline

| Component | Minimum | Recommended | Tested on |
|-----------|---------|-------------|-----------|
| GPU | 6 GB VRAM (CUDA or Metal) | 8 GB VRAM (qwen3.5 path) — or 16 GB+ for the qwen3.6 path | RTX 3060 Ti 8 GB / RTX 5060 Ti 16 GB |
| RAM | 16 GB | 32 GB | 32 GB DDR4 |
| Disk | ~3 GB (brain only) — or ~8 GB with the lightweight 7b coder | ~27 GB (3.5 brain + fallback + 30b coder) / ~14 GB (single byteshape qwen3.6-35b MTP quant serving all roles) | NVMe SSD |
| OS | Windows 10+, Linux, macOS | Windows 11 / Ubuntu 24.04 / macOS 14+ | Windows 11 Pro |

> **Which model should I pick?** Short answer: the tier table in the README's
> [Which model should I run?](../README.md#-which-model-should-i-run). TL;DR:
> `qwen3.5:4b` for the smallest setup (8 GB GPU or plain CPU); on a 16 GB GPU the
> measured best is **`byteshape/qwen3.6-35b-a3b-mtp`** via LM Studio — 50/50 on the
> battery at ~70–76 tok/s, fully VRAM-resident. The rest of this page covers how to
> choose and how to load it right.

## Which model for which GPU (measured)

All numbers measured 2026-07-11 on claudette v0.16.0, LM Studio 0.4.19 (runtime
cuda12-avx2 2.24.0), RTX 5060 Ti 16 GB, on the 50-task battery ("K" is a separate
8-task new-language section). Full tables and methodology:
[MODEL-COMPARISON.md](../runs/eval-2026-05-29/battery/MODEL-COMPARISON.md); per-config
checkpoints, decision matrix, and launch crib sheet:
[CHAMPION-DOSSIER.md](../runs/eval-2026-05-29/battery/CHAMPION-DOSSIER.md).

| Your GPU | Pick | Battery | Speed | Notes |
|----------|------|---------|-------|-------|
| **16 GB (best)** | `byteshape/qwen3.6-35b-a3b-mtp` (ShapeLearn 3.06 bpw, 13.6 GB) | **50/50 + K 8/8** @24k ctx · 49/50 + K 8/8 @64k | ~70–76 tok/s gen, full battery in 10.1 min | Fully VRAM-resident (~15.4 GiB @64k ctx), zero RAM spill. Community quantizer (not unsloth/official); bundles an MTP draft head — load flags below. The one 64k miss was one-task variance, not a pattern |
| 16 GB, official-lineage alt | `qwen3.6-35b-a3b@iq4_xs` (unsloth UD-IQ4_XS, 17.7 GB) | **50/50 + K 8/8** | 27.8 tok/s | First-ever perfect core-50; equal quality from the unsloth line, but spills to RAM and IQ-family kernels dequant slowly on the CPU-expert path |
| 16 GB, previous default | `qwen3.6-35b-a3b@q3_k_xl` (unsloth UD-Q3_K_XL, 16.8 GB) | 47/50 + K 8/8 | 33.8 tok/s | Known-good rollback: `lms load "qwen3.6-35b-a3b@q3_k_xl" -c 65536 --parallel 1 -y` |
| **8 GB or plain CPU** | `qwen3.5:4b` | 45/50 (90%) + K 8/8 | full battery in 12.8 min | Best value; the battery ran the 7.3 GB LM Studio build — the Ollama `qwen3.5:4b` pull is ~3.4 GB |
| Fastest / lowest overhead | `gpt-oss-20b` (~13 GB) | 41/50 (82%) + K 7/8 | full battery in 6.1 min | Quickest full run; signature weakness is multi-site refactor/rename (does the first edit, leaves the rest) |
| 24 GB+ | untested on our rig | — | — | We only have a 16 GB card. Likely paths: unsloth UD-Q4_K_XL and up for quality, higher-bpw byteshape MTP tiers for speed — a benchmark report is the most useful contribution |

## Choosing a model — what actually matters

1. **VRAM residency beats parameter count and bits-per-weight.** The 13.6 GB
   3.06 bpw quant that fits entirely in VRAM beats every bigger, higher-bpw quant of
   the *same model* on speed (2–3×) at equal-or-better battery quality. Before
   chasing a bigger quant, check it actually fits your card.
2. **KV cache `q8_0` and `--parallel 1`, always.** Quantised KV halves cache memory
   with no measured quality loss on the battery. `--parallel 1` is essential:
   with N>1 slots, llama.cpp/LM Studio splits the context window N ways, silently
   starving long-context tasks.
3. **MTP (multi-token prediction / speculative decoding) only pays when the quant is
   fully VRAM-resident** — at least in LM Studio. On a spilled quant, draft acceptance
   stays high (90%+) but the verify batch pays the PCIe tax twice, netting ~zero.
4. **If a quant must spill to RAM, prefer Q_K-family over IQ-family** — IQ quants
   dequant slowly on the CPU-expert path. Resident IQ is fine.
5. **Template health flips with runtime versions — in both directions.** LM Studio
   runtime cuda12-avx2 2.24.0 fixed two models the previous sweep had to gate out and
   *broke* `qwen3.5:9b` (emits an empty first turn when tools are in the system
   prompt — both the `qwen/` and `unsloth/` builds; don't pick 9b on that runtime).
   After any runtime upgrade, smoke-test your model with one tool-using prompt
   before trusting it.

## Loading the 16 GB champion (LM Studio)

```sh
lms load "byteshape/qwen3.6-35b-a3b-mtp" -c 65536 --parallel 1 \
    --speculative-draft-mtp --speculative-draft-max-tokens 2 -y
```

- `--speculative-draft-max-tokens 2` is the measured peak (3 and 4 both lose ~4%).
- Per-model settings that should be pinned (LM Studio per-model defaults or the load
  command): **ctx 65536 · KV cache q8_0 (K and V) · no-mmap · 0 CPU experts ·
  `--parallel 1`**.
- Forgot the MTP flags, or claudette JIT-loaded the model? Still fine — MTP in
  LM Studio adds only ~2–13% on this resident quant; residency is the real win.
- Claudette side: `CLAUDETTE_MODEL=byteshape/qwen3.6-35b-a3b-mtp` with
  `CLAUDETTE_OPENAI_COMPAT=1` — see [`configuration.md`](configuration.md).

## Spilled quants: the llama-server fit-target path

If you run a quant that *can't* fit VRAM (e.g. unsloth Q4_K_XL on a 16 GB card), LM
Studio's MTP won't help (point 3 above) — but a llama.cpp `llama-server` build with
`--fit-target 2304` + the MTP draft head reached **43.1 tok/s** on the MTP Q4_K_XL,
the fastest spilled-quant config we measured. Pass `--jinja` or tool-calling silently
degrades. Setup notes: [MODEL-COMPARISON.md](../runs/eval-2026-05-29/battery/MODEL-COMPARISON.md)
(champion-tuning section).

## Model footprint

| Model | Role | VRAM | Throughput |
|-------|------|------|------------|
| `qwen3.5:4b` | Brain (default) | ~3.4 GB | ~55 t/s on 3060 Ti |
| `qwen3.5:9b` | Fallback brain | ~5.5 GB | ~30 t/s on 3060 Ti — ⚠ template-broken on LM Studio runtime 2.24.0 (see above); Ollama path untested on that regression |
| **`byteshape/qwen3.6-35b-a3b-mtp`** | **Brain (recommended, 16 GB)** | 13.6 GB on disk, ~15.4 GiB resident @64k ctx | ~70–76 tok/s on RTX 5060 Ti 16 GB |

The 4b brain alone is viable as a standalone setup — it handles coding, tool-calling, note-taking, calendar, and conversation perfectly fine on its own. Add the 9b (or move to the 35b) only when you want better multi-step reasoning.

For the **qwen3.6 path** (recommended on 16 GB+ VRAM), a single model serves everything — no fallback pull needed. Currently distributed via LM Studio (byteshape or Unsloth GGUF). See [`power-user.md`](power-user.md#lm-studio-or-any-openai-compatible-server) for backend setup. (In `--forge` mode you can still route the Coder/Verifier roles to different models via `~/.claudettes-forge/models.toml` — see [`forge.md`](forge.md).)

## Running a large brain on 8 GB VRAM / 32 GB RAM

To run a big MoE brain (e.g. the 35b) on a constrained box, set these Ollama env vars before launching `ollama serve`:

```bash
OLLAMA_MAX_LOADED_MODELS=1    # keep one model resident at a time
OLLAMA_FLASH_ATTENTION=1      # halves the KV cache
OLLAMA_KV_CACHE_TYPE=q8_0     # quantised KV cache
```

## No GPU? CPU-only mode

You don't need a discrete GPU to run Claudette. Ollama happily runs the default brain on plain CPU — at lower throughput, but enough to be useful for assistant work (notes, calendar, weather, brief Q&A) and short coding turns. The larger MoE models are not realistic on CPU; everything else is.

One command, same default the installer suggests:

```sh
ollama pull qwen3.5:4b     # ~3.4 GB
```

`qwen3.5:4b` scores **45/50 (90%) + K 8/8** on the battery. That score was measured on the GPU rig above — the model gives the same answers on CPU, just slower. We have **not** measured CPU tok/s on the battery yet, so the table below gives honest qualitative expectations, not numbers; a CPU-only benchmark report is one of the most useful contributions you can make.

| Hardware | Brain that fits | Expectation (unmeasured) |
|----------|-----------------|--------------------------|
| MacBook Air M1/M2 (8 GB unified) | `qwen3.5:4b` | conversational — fine as a daily assistant |
| Intel laptop with 16 GB RAM, no GPU | `qwen3.5:4b` | usable, not snappy |
| Raspberry Pi 5 (8 GB) | `qwen2.5:1.5b` | short prompts / Telegram-bot duty |
| Old desktop, 8 GB RAM, no GPU | `qwen2.5:0.5b`–`1.5b` | short prompts only |

If you drop below the 4b, pin the smaller brain from inside Claudette:

```
/brain qwen2.5:1.5b
```

…or set it permanently in `~/.claudette/.env`:

```
CLAUDETTE_MODEL=qwen2.5:1.5b
```

Pull the model once with `ollama pull <model>`. Ollama auto-detects no-GPU and uses CPU; no configuration required on its side.

**Trade-offs you should know:**

- The 0.5b and 1.5b brains will hallucinate tool-call shapes more often than the 4b. The auto-fix loops catch most of it but you'll occasionally see retries in the TUI's Tools tab.
- `--forge` mode is realistic on CPU only with a small brain and a lot of patience — the autonomous plan→code→verify→fix loop runs many turns. Prefer it on a GPU box.
- First-token latency is the noticeable cost — model load time. Keep the same brain hot between turns; the load-time warmup is per-cold-start, not per-turn. The REPL prints a one-time heads-up on the first request of a session so a cold load never reads as a silent hang.

If your machine is too small even for the 0.5b brain, Claudette is the wrong tool — at that point you want a hosted service (ChatGPT, Claude.ai). The whole *point* of Claudette is local inference; there is no cloud fallback.

## Presets

Claudette ships with three brain presets:

- **Fast**: brain is `qwen3.5:4b` (fast, 3.4 GB VRAM), no fallback.
- **Auto** (default): `qwen3.5:4b` with auto-escalation to `qwen3.5:9b` on stuck signals (empty response after retry, max iterations hit with no text, ≥ 3 consecutive tool errors). Reverts to 4b after the failed turn — per-turn revert, not session-sticky.
- **Smart**: brain is `qwen3.5:9b`, no fallback.

Switch at runtime with `/preset fast | auto | smart`, or pin a specific brain with `/brain <model>`.
