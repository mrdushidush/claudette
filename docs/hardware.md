# Hardware requirements

## Baseline

| Component | Minimum | Recommended | Tested on |
|-----------|---------|-------------|-----------|
| GPU | 6 GB VRAM (CUDA or Metal) | 8 GB VRAM (qwen3.5 path) — or 16 GB+ for the qwen3.6 path | RTX 3060 Ti 8 GB / RTX 5060 Ti 16 GB |
| RAM | 16 GB | 32 GB | 32 GB DDR4 |
| Disk | ~3 GB (brain only) — or ~8 GB with the lightweight 7b coder | ~27 GB (3.5 brain + fallback + 30b coder) / ~24 GB (single qwen3.6-35b-a3b serving both roles) | NVMe SSD |
| OS | Windows 10+, Linux, macOS | Windows 11 / Ubuntu 24.04 / macOS 14+ | Windows 11 Pro |

> **Which model should I pick?** See [Recommended models](../README.md#recommended-models) in the README for the per-tier hierarchy. TL;DR: `qwen3.5:4b` for the smallest setup; **`qwen3.6-35b-a3b` (via LM Studio) for the best brain & coder by a wide margin** when you have 16 GB+ VRAM or 32 GB RAM with CPU-MoE offload.

## Model footprint

| Model | Role | VRAM | Throughput |
|-------|------|------|------------|
| `qwen3.5:4b` | Brain (default) | ~3.4 GB | ~55 t/s on 3060 Ti |
| `qwen3.5:9b` | Fallback brain | ~5.5 GB | ~30 t/s on 3060 Ti |
| `qwen3-coder:30b` | Codet coder (quality, default) | ~19 GB total (MoE, partial RAM spill) | ~20 t/s effective on 3060 Ti |
| `qwen2.5-coder:14b` | Codet coder (fallback) | ~9 GB | ~8 t/s with partial spill |
| `qwen2.5-coder:7b` | Codet coder (lightweight) | ~4.5 GB | ~30 t/s on 3060 Ti |
| **`qwen3.6-35b-a3b`** (Q4_K_XL) | **Brain & coder (recommended)** | ~24 GB total — MoE, ~3 GB active in VRAM + RAM for inactive experts via `--cpu-moe` | ~24 t/s on RTX 5060 Ti, ~43 t/s with MTP speculative decoding |
| `qwen3.6-27b` (dense, Q4) | Coder option (top quality) | ~17 GB VRAM — **very tight on 16 GB**, comfortable on 24 GB+ | untested by us; expect lower than 35b-a3b on Q4 due to dense architecture |

The 4b brain alone is viable as a standalone setup — it handles tool-calling, note-taking, calendar, and conversation perfectly fine on its own. Add the 9b only when you want better multi-step reasoning. Add a coder only when you use `generate_code`; the 7b fits happily alongside the 4b on 8 GB VRAM.

For the **qwen3.6 path** (recommended on 16 GB+ VRAM), one model serves both brain and coder — no swap dance, no fallback pull needed. Currently distributed via LM Studio (Unsloth GGUF). See [`power-user.md`](power-user.md#lm-studio-or-any-openai-compatible-server) for backend setup.

## Running the 30b coder on 8 GB VRAM / 32 GB RAM

Set these Ollama env vars before launching `ollama serve`:

```bash
OLLAMA_MAX_LOADED_MODELS=1    # forces brain eviction before coder loads
OLLAMA_FLASH_ATTENTION=1      # halves the KV cache
OLLAMA_KV_CACHE_TYPE=q8_0     # quantised KV cache
```

With these settings, the 4b brain gets evicted from VRAM when Codet needs the 30b coder, then restored after Codet finishes. Swap cost is ~5–10 seconds on a 3060 Ti.

## No GPU? CPU-only mode

You don't need a discrete GPU to run Claudette. Ollama happily runs the smaller Qwen models on plain CPU — at lower throughput, but enough to be useful for short-turn assistant work (notes, calendar, weather, brief Q&A). The big-coder models are not realistic on CPU; everything else is.

What to expect:

| Hardware | Brain that fits | Realistic throughput |
|----------|-----------------|----------------------|
| MacBook Air M1/M2 (8 GB unified) | `qwen2.5:3b` (~2 GB) | ~12–18 t/s — perfectly conversational |
| Intel laptop with 16 GB RAM, no GPU | `qwen2.5:1.5b` or `qwen2.5:3b` | ~5–10 t/s — usable, not snappy |
| Raspberry Pi 5 (8 GB) | `qwen2.5:0.5b` or `1.5b` | ~3–6 t/s — best for the Telegram-bot use case |
| Old desktop, 8 GB RAM, no GPU | `qwen2.5:0.5b` | ~4–8 t/s — works for short prompts |

Pick a smaller brain and pin it. From inside Claudette:

```
/brain qwen2.5:3b
```

…or set it permanently in `~/.claudette/.env`:

```
CLAUDETTE_MODEL=qwen2.5:3b
```

Pull the model once with `ollama pull qwen2.5:3b` (or whichever size matches your machine). Ollama auto-detects no-GPU and uses CPU; no configuration required on its side.

**Trade-offs you should know:**

- The 0.5b and 1.5b brains will hallucinate tool-call shapes more often than the 4b. The auto-fix loops catch most of it but you'll occasionally see retries in the TUI's Tools tab.
- Codet (`generate_code`) and `--forge` mode are not realistic on CPU — they assume a coder model. Skip them, or set Codet's coder to `qwen2.5-coder:1.5b` and accept very slow generation.
- First-token latency is the noticeable cost — model load time. Keep the same brain hot between turns; the 4-5 second warmup is per-cold-start, not per-turn.

If your machine is too small even for the 0.5b brain, Claudette is the wrong tool — at that point you want a hosted service (ChatGPT, Claude.ai). The whole *point* of Claudette is local inference; there is no cloud fallback.

## Presets

Claudette ships with three brain presets:

- **Fast**: brain is `qwen3.5:4b` (fast, 3.4 GB VRAM), no fallback.
- **Auto** (default): `qwen3.5:4b` with auto-escalation to `qwen3.5:9b` on stuck signals (empty response after retry, max iterations hit with no text, ≥ 3 consecutive tool errors). Reverts to 4b after the failed turn — per-turn revert, not session-sticky.
- **Smart**: brain is `qwen3.5:9b`, no fallback.

Switch at runtime with `/preset fast | auto | smart`, or pin a specific brain with `/brain <model>`.
