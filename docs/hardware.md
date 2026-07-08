# Hardware requirements

## Baseline

| Component | Minimum | Recommended | Tested on |
|-----------|---------|-------------|-----------|
| GPU | 6 GB VRAM (CUDA or Metal) | 8 GB VRAM (qwen3.5 path) — or 16 GB+ for the qwen3.6 path | RTX 3060 Ti 8 GB / RTX 5060 Ti 16 GB |
| RAM | 16 GB | 32 GB | 32 GB DDR4 |
| Disk | ~3 GB (brain only) — or ~8 GB with the lightweight 7b coder | ~27 GB (3.5 brain + fallback + 30b coder) / ~24 GB (single qwen3.6-35b-a3b serving both roles) | NVMe SSD |
| OS | Windows 10+, Linux, macOS | Windows 11 / Ubuntu 24.04 / macOS 14+ | Windows 11 Pro |

> **Which model should I pick?** See [Recommended models](../README.md#recommended-models) in the README for the per-tier hierarchy. TL;DR: `qwen3.5:4b` for the smallest setup; **`qwen3.6-35b-a3b` (via LM Studio) for the best brain by a wide margin** when you have 16 GB+ VRAM or 32 GB RAM with CPU-MoE offload.

## Model footprint

| Model | Role | VRAM | Throughput |
|-------|------|------|------------|
| `qwen3.5:4b` | Brain (default) | ~3.4 GB | ~55 t/s on 3060 Ti |
| `qwen3.5:9b` | Fallback brain | ~5.5 GB | ~30 t/s on 3060 Ti |
| **`qwen3.6-35b-a3b`** (Q4_K_XL) | **Brain (recommended)** | ~24 GB total — MoE, ~3 GB active in VRAM + RAM for inactive experts via `--cpu-moe` | ~24 t/s on RTX 5060 Ti, ~43 t/s with MTP speculative decoding |

The 4b brain alone is viable as a standalone setup — it handles coding, tool-calling, note-taking, calendar, and conversation perfectly fine on its own. Add the 9b (or move to the 35b) only when you want better multi-step reasoning.

For the **qwen3.6 path** (recommended on 16 GB+ VRAM), a single model serves everything — no fallback pull needed. Currently distributed via LM Studio (Unsloth GGUF). See [`power-user.md`](power-user.md#lm-studio-or-any-openai-compatible-server) for backend setup. (In `--forge` mode you can still route the Coder/Verifier roles to different models via `~/.claudettes-forge/models.toml` — see [`forge.md`](forge.md).)

## Running a large brain on 8 GB VRAM / 32 GB RAM

To run a big MoE brain (e.g. the 35b) on a constrained box, set these Ollama env vars before launching `ollama serve`:

```bash
OLLAMA_MAX_LOADED_MODELS=1    # keep one model resident at a time
OLLAMA_FLASH_ATTENTION=1      # halves the KV cache
OLLAMA_KV_CACHE_TYPE=q8_0     # quantised KV cache
```

## No GPU? CPU-only mode

You don't need a discrete GPU to run Claudette. Ollama happily runs the smaller Qwen models on plain CPU — at lower throughput, but enough to be useful for short-turn assistant work (notes, calendar, weather, brief Q&A). The larger MoE models are not realistic on CPU; everything else is.

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
- `--forge` mode is realistic on CPU only with a small brain and a lot of patience — the autonomous plan→code→verify→fix loop runs many turns. Prefer it on a GPU box.
- First-token latency is the noticeable cost — model load time. Keep the same brain hot between turns; the 4-5 second warmup is per-cold-start, not per-turn.

If your machine is too small even for the 0.5b brain, Claudette is the wrong tool — at that point you want a hosted service (ChatGPT, Claude.ai). The whole *point* of Claudette is local inference; there is no cloud fallback.

## Presets

Claudette ships with three brain presets:

- **Fast**: brain is `qwen3.5:4b` (fast, 3.4 GB VRAM), no fallback.
- **Auto** (default): `qwen3.5:4b` with auto-escalation to `qwen3.5:9b` on stuck signals (empty response after retry, max iterations hit with no text, ≥ 3 consecutive tool errors). Reverts to 4b after the failed turn — per-turn revert, not session-sticky.
- **Smart**: brain is `qwen3.5:9b`, no fallback.

Switch at runtime with `/preset fast | auto | smart`, or pin a specific brain with `/brain <model>`.
