# Hardware requirements

## Baseline

| Component | Minimum | Recommended | Tested on |
|-----------|---------|-------------|-----------|
| GPU | 6 GB VRAM (CUDA or Metal) | 8 GB VRAM | RTX 3060 Ti 8 GB |
| RAM | 16 GB | 32 GB | 32 GB DDR4 |
| Disk | ~3 GB (brain only) — or ~8 GB with the lightweight 7b coder | ~27 GB (brain + fallback + 30b coder) | NVMe SSD |
| OS | Windows 10+, Linux, macOS | Windows 11 / Ubuntu 24.04 / macOS 14+ | Windows 11 Pro |

## Model footprint

| Model | Role | VRAM | Throughput (3060 Ti) |
|-------|------|------|----------------------|
| `qwen3.5:4b` | Brain (default) | ~3.4 GB | ~55 t/s |
| `qwen3.5:9b` | Fallback brain | ~5.5 GB | ~30 t/s |
| `qwen3-coder:30b` | Codet coder (quality) | ~19 GB total (MoE, partial RAM spill) | ~20 t/s effective |
| `qwen2.5-coder:14b` | Codet coder (fallback) | ~9 GB | ~8 t/s with partial spill |
| `qwen2.5-coder:7b` | Codet coder (lightweight) | ~4.5 GB | ~30 t/s |

The 4b brain alone is viable as a standalone setup — it handles tool-calling, note-taking, calendar, and conversation perfectly fine on its own. Add the 9b only when you want better multi-step reasoning. Add a coder only when you use `generate_code`; the 7b fits happily alongside the 4b on 8 GB VRAM.

## Running the 30b coder on 8 GB VRAM / 32 GB RAM

Set these Ollama env vars before launching `ollama serve`:

```bash
OLLAMA_MAX_LOADED_MODELS=1    # forces brain eviction before coder loads
OLLAMA_FLASH_ATTENTION=1      # halves the KV cache
OLLAMA_KV_CACHE_TYPE=q8_0     # quantised KV cache
```

With these settings, the 4b brain gets evicted from VRAM when Codet needs the 30b coder, then restored after Codet finishes. Swap cost is ~5–10 seconds on a 3060 Ti.

## Presets

Claudette ships with three brain presets:

- **Fast**: brain is `qwen3.5:4b` (fast, 3.4 GB VRAM), no fallback.
- **Auto** (default): `qwen3.5:4b` with auto-escalation to `qwen3.5:9b` on stuck signals (empty response after retry, max iterations hit with no text, ≥ 3 consecutive tool errors). Reverts to 4b after the failed turn — per-turn revert, not session-sticky.
- **Smart**: brain is `qwen3.5:9b`, no fallback.

Switch at runtime with `/preset fast | auto | smart`, or pin a specific brain with `/brain <model>`.
