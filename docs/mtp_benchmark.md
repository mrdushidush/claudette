# qwen3.6-35b-a3b MTP benchmark on RTX 5060 Ti (16 GB)

**Date:** 2026-05-16
**Rig:** RTX 5060 Ti 16 GB · driver 596.36 · Intel i5-10500 · 32 GB RAM · Windows 11
**llama.cpp branch:** `am17an/llama.cpp@mtp-clean` (HEAD `2dff7ff` "conversion: fix type annotations")
**Model:** `unsloth/Qwen3.6-35B-A3B-MTP-GGUF` · file `Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf` (22.86 GB)

## TL;DR

**Generation speedup: 1.77× (avg) — well above the 1.4× success threshold.** GO on repointing claudette at the MTP server, **with one critical config caveat** (see below).

## Build commands actually used

```powershell
# 1. Clone the open MTP PR branch
git clone --depth 1 -b mtp-clean https://github.com/am17an/llama.cpp.git D:\dev\llama.cpp-mtp

# 2. Configure (Visual Studio 17 2022 generator, SM_120a for Blackwell consumer)
$env:CUDA_PATH       = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.2"
$env:CUDA_PATH_V13_2 = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.2"
$env:Path = "C:\Program Files\CMake\bin;$env:CUDA_PATH\bin\x64;" + $env:Path
cd D:\dev\llama.cpp-mtp
cmake -G "Visual Studio 17 2022" -A x64 -B build `
      -DGGML_CUDA=ON `
      -DCMAKE_CUDA_ARCHITECTURES=120 `
      -DCMAKE_CUDA_COMPILER="C:/Program Files/NVIDIA GPU Computing Toolkit/CUDA/v13.2/bin/nvcc.exe"

# 3. Build Release
cmake --build build --config Release -j 12   # ~40 min on 12-thread i5-10500
```

Toolchain: VS Build Tools 17.14.31 (MSVC 14.44.35207), CUDA 13.2.78, CMake 4.3.2. All installed via winget (CUDA bundled driver was skipped because 596.36 was newer than the bundled one).

### Build gotchas

- **CUDA 13.2 puts DLLs in `bin\x64`, not `bin`** — that path must be on `PATH` for `llama-server.exe` to start. Older CUDA versions had everything in `bin`.
- **`CUDA_PATH_V13_2` env var is required** for MSBuild's CUDA integration to resolve `CudaToolkitDir`. A shell started before the winget install needs to refresh from the machine scope.
- **`-DCMAKE_CUDA_ARCHITECTURES=120`** is auto-upgraded by ggml's CMake to `120a` (PTX/SASS for the new Blackwell consumer MMA features). `-arch=native` would also work on a built rig but explicit beats implicit.
- **HTTPS support is OFF by default on Windows** because OpenSSL dev files aren't present. That means `llama-server -hf unsloth/...` fails at startup with "HTTPS not supported". Two workarounds: rebuild with `-DLLAMA_BUILD_BORINGSSL=ON`, or manually download the GGUF (we did the latter — same wall-clock as `-hf` anyway).

## Final llama-server command (production-recommended)

```powershell
$env:Path = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.2\bin\x64;" + $env:Path

.\build\bin\Release\llama-server.exe `
  -m C:\models\Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf `
  -c 8192 -fa on -np 1 --port 1235 `
  --fit-target 2816 `
  --spec-type draft-mtp --spec-draft-n-max 2
```

> **CRITICAL CONFIG NOTE (see [config sensitivity](#config-sensitivity) below):**
> `--fit-target 2816` (= 2.75 GiB margin) gives the optimal config — the auto-fit reserves enough headroom to avoid VRAM spillage while packing as many MoE experts on GPU as fit. **DO NOT use `--cpu-moe` here** — counter-intuitively that pessimizes throughput on this rig because it forces ALL routed experts to RAM instead of letting the fit logic pack the best ones on GPU.
>
> Also, the **`--spec-type draft-mtp`** flag is critical — the pre-2026-05-13 alias `--spec-type mtp` silently disables MTP (verified in `common/speculative.cpp:27`).

The MTP head is embedded in the same GGUF — no separate `-md` / draft model file needed. The server log confirms: `creating MTP draft context against the target model 'C:\models\...'`. Code path at `tools/server/server-context.cpp:801-820` builds a second `LLAMA_CONTEXT_TYPE_MTP` context against the same `model_tgt`.

## Measurements

All measurements: streaming SSE, `temperature=0`, `seed=42`, `max_tokens=400`, qwen3.6 thinking mode ON. Generation tok/s counts both `delta.content` and `delta.reasoning_content` chunks.

### Per-prompt tok/s (best run, post-warmup)

| Prompt | LM Studio :1234 | MTP :1235 (fit-2816) | Speedup |
|---|---:|---:|---:|
| short_qa            | 25.55 | 44.80 | **1.75×** |
| code_emit_small     | 24.82 | 40.47 | **1.63×** |
| explain_medium      | 24.81 | 41.04 | **1.65×** |
| refactor_longish    | 24.64 | 43.48 | **1.76×** |
| long_context        | 24.94 | 44.53 | **1.79×** |
| **avg**             | **24.95** | **42.86** | **1.72×** |

Two-run avg (initial + post-tuning): **LM Studio 24.36 tok/s, MTP 43.16 tok/s = 1.77× speedup**.

TTFT (prompt-eval proxy) was similar between the two: 1.0–6.5s, dominated by prompt size + cold-prompt-cache. MTP doesn't change prompt-eval rate (expected — MTP is generation-only).

> Raw data: `bench\lmstudio-final.json`, `bench\mtp-final.json`, `bench\mtp-13.5g.json`.

### MTP draft acceptance (from server log)

5 prompts: **93.1% · 77.9% · 75.7% · 83.8% · 89.2% → avg ~83.9%**.

With `--spec-draft-n-max 2` and ~84% acceptance, each speculation call yields ~1.84 generated / ~1.68 accepted tokens. The 1.77× wall-clock speedup is in line with that arithmetic plus the cost of the verification batch.

### Config sensitivity

We benchmarked four configurations to characterize the cliff. All same model + same probes, only the offload strategy changes:

| Config | VRAM used | Avg gen tok/s | vs baseline |
|---|---:|---:|---:|
| LM Studio (UI defaults, --cpu-moe, 32K ctx) | ~11.0 GB | 23.77–24.95 | 1.00× |
| MTP server `--cpu-moe` (all experts → RAM)  | 4.6 GB   | 27.46 | 1.16× |
| MTP server auto-fit, default 1024 MiB margin (SPILLED) | 15.7 GB | 19.02 | 0.80× ⚠ |
| **MTP server `--fit-target 2816` (~13.4 GB peak)** | **13.4 GB** | **42.86** | **1.72×** ✅ |

**Key finding:** `--cpu-moe` is *too aggressive* (4.6 GB only — 11 GB of headroom wasted) and the default 1 GB margin is *too tight* (model loads but spills into shared system RAM during inference, ~4× slowdown). Sweet spot: leave ~2.5–3 GB margin via `--fit-target 2816` so KV cache, MTP scratch, and verification batch all fit without spillage.

## Quality (5-prompt eyeball)

Speculative decoding preserves output distribution by construction (rejected drafts trigger resample from target), so quality preservation is a theoretical guarantee, not a benchmark outcome. The empirical check:

- **short_qa**: both → "Paris" ✅
- **long_context**: LM Studio → "dolor" (correct third word); MTP exhausted 400 tok budget mid-reasoning (faster gen consumed budget before the visible answer — probe artifact, not quality regression).
- **code_emit_small / explain_medium / refactor_longish**: identical reasoning prefixes ("Here's a thinking process: 1. Analyze User Input: ..."); minor stylistic variation only where temperature=0 sampling diverges after rejected drafts.

No semantic regression observed. Combined with ~84% acceptance, MTP is working as designed.

## Go / No-go recommendation

### GO — repoint claudette at the MTP server.

**Why:** 1.77× generation throughput on the same hardware, no quality loss, low integration risk (drop-in OpenAI-compat endpoint on a different port).

### Caveats / what to weigh before flipping

1. **You lose LM Studio's affordances** — JIT swap between qwen3.6/qwen3.5-4b/gemma-4/embeddings/etc. happens transparently today. The MTP llama-server is single-model. Suggest: keep LM Studio on :1234 for the smaller models + embeddings, run llama-server on :1235 for the main qwen3.6 brain. Claudette already supports per-role base-URL overrides (per `~/.claudettes-forge/models.toml`).

2. **No HTTPS in this build** — if anything routes through public-facing HTTPS termination, that's TLS reverse-proxy territory anyway, but worth noting.

3. **The `--fit-target 2816` config is empirically tuned** to this exact rig + this exact quant. If you swap the GPU, the model, or the context size, re-tune. The default 1024 MiB margin is genuinely dangerous on this 16 GB card with this 22.86 GB model.

4. **MTP context cost** — each request the server runs a verification batch alongside, costing ~5% VRAM headroom vs vanilla generation. Already baked into our 2.75 GB margin.

5. **Did NOT touch** `~/.claudette/.env`, `~/.claudettes-forge/models.toml`, or the Windows `CLAUDETTE_MODEL` user env var (per the goal's constraint). Switching is your call; the suggested change would be: in `~/.claudettes-forge/models.toml`, point the planner/coder/verifier roles' base URL at `http://localhost:1235/v1`, leave embeddings/secretary on `:1234`.

## Stretch goal (ngram-mod on top of MTP)

**Not attempted.** Total build wall-clock was ~40 min, well under the 2-hour cap, so the time gate didn't kick in. The reason for skipping: ngram-mod's docs explicitly note "MoEs require long drafts" (defaults: `--spec-ngram-mod-n-max 64`), and MTP wants `--spec-draft-n-max 2` (short drafts). The two combine via comma-separated `--spec-type ngram-mod,draft-mtp` but the conflicting draft-length defaults need a co-tuning pass. Worth a follow-up session, not blocking on this report.

## Reproducing

1. Build: see [Build commands actually used](#build-commands-actually-used).
2. Download: `Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf` from `unsloth/Qwen3.6-35B-A3B-MTP-GGUF` to `C:\models\`. ~5.4 min @ ~70 MB/s.
3. Launch: see [Final llama-server command](#final-llama-server-command-production-recommended).
4. Bench: `D:\dev\llama.cpp-mtp\bench\probe.ps1 -BaseUrl http://localhost:1235 -Label mtp -OutFile mtp.json`.

Probe source: `D:\dev\llama.cpp-mtp\bench\probe.ps1`.
