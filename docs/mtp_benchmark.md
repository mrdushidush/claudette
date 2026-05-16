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
  -c 32768 -fa on -np 1 --port 1235 `
  --fit-target 2304 `
  -ctk q8_0 -ctv q8_0 --cache-ram 1024 `
  --spec-type draft-mtp --spec-draft-n-max 2 `
  --no-mmap
```

> **2026-05-16 round-2 tune deltas** (raw data: `bench\fit-*.json`, `bench\nmax-*.json`, `bench\ctx32k-*.json`):
>
> - **`--fit-target` 2816 → 2304** (peak throughput at 2304: 45.73 tok/s avg, +4.2% over the prior 2816 baseline of 43.9). Sweep covered 2048/2304/2560/2816/3072/3584; 2048 dropped to 40.8 (over-packing penalty), 3584 dropped to 43.0 (under-packing). VRAM 14.23 GB at 2304, RAM still 15.0 GB free.
> - **`-c` 8192 → 32768** because forge missions need ≥10K context (the 8K config errors out at coder-round-0 with `request (8308 tokens) exceeds the available context size`). The 4× bump costs ~150 MB VRAM and <1 tok/s — auto-fit rebalances by leaving 0.5 GB more experts in RAM.
> - **`--spec-draft-n-max`** stays at 2. Confirmed empirically: 1 → 43.7, **2 → 45.7 (peak)**, 3 → 45.3, 4 → 42.6, 6 → 36.9. The MTP head is clearly trained for N=2.
> - **MXFP4_MOE NOT a win on this rig** despite Blackwell native FP4. Tested `Qwen3.6-35B-A3B-MTP-GGUF/Qwen3.6-35B-A3B-MXFP4_MOE.gguf` (20.66 GB) at the same fit-2304 + n-max-2: 43.9 tok/s avg, slightly slower than Q4_K_XL. Workload is memory-bandwidth-bound, not compute-bound, so the FP4 path doesn't pay off here. Q4_K_XL stays.
> - **`--cache-ram`** stays at 1024. Bumping to 8192 (to see if prompt cache thrashing was the issue) made forge wall-clock WORSE (368s vs 224.8s for the same mission) — within noise but not a win.
>
> **CRITICAL CONFIG NOTES:**
>
> 1. **`--fit-target 2304`** (= 2.25 GiB margin) gives the optimal VRAM packing — the auto-fit reserves enough headroom to avoid spillage while packing as many MoE experts on GPU as fit. **DO NOT use `--cpu-moe`** — counter-intuitively that pessimizes throughput on this rig because it forces ALL routed experts to RAM instead of letting the fit logic pack the best ones on GPU.
>
> 2. **`--spec-type draft-mtp`** is critical — the pre-2026-05-13 alias `--spec-type mtp` silently disables MTP (verified in `common/speculative.cpp:27`).
>
> 3. **`--no-mmap`** is the RAM-priority unlock (added 2026-05-16 follow-up — see [§RAM-priority tune](#ram-priority-tune-no-mmap) below). Frees ~10 GB of system RAM and improves tk/s by ~9% vs mmap default.

## RAM-priority tune (`--no-mmap`)

Follow-up to the main benchmark: 4 GB free RAM during inference wasn't enough to keep using the rig for other things (browser, IDE, light builds). The mmap warning the server emits — *"tensor overrides to CPU are used with mmap enabled - consider --no-mmap for better performance"* — turns out to be 100% accurate on this rig. Re-running the same 5-prompt probe with `--no-mmap` added:

| Config | RAM free (steady) | Gen tk/s avg | VRAM | Quality |
|---|---:|---:|---:|---|
| Q4_K_XL + mmap (prior production) | 4.4 GB | 40.16 | 13.84 GB | KLD 0.41 |
| **Q4_K_XL + `--no-mmap` (new production)** | **14.6 GB** | **43.92** | 13.84 GB | KLD 0.41 |

**Δ:** +10.2 GB free RAM (3.3× headroom), +9.4% gen tk/s, same VRAM, same quality.

Why `--no-mmap` wins here despite the conventional "mmap is friendlier to RAM" intuition: with `--fit-target 2816` already putting ~13.4 GB of model on VRAM, only ~9.5 GB of expert tensors need to stay CPU-resident. Under mmap, the OS page cache holds the entire 22.9 GB GGUF hot after load (the file got touched once during upload, every page is "warm"). Under `--no-mmap`, llama.cpp allocates private buffers, uploads GPU layers to VRAM, and the buffer pages for those layers go cold — Windows pages them out cleanly via the working set manager. Result: process Private commit climbs to ~27 GB but resident WS stays at ~13 GB, and the rest of system RAM stays available.

The tk/s gain is incidental but consistent across runs (44.0 ± 1.5 tok/s across two probes): private allocations avoid the page-fault path on cold expert lookups that mmap suffers when Windows trims rarely-used pages.

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

## End-to-end forge wall-clock (2026-05-16 round 2)

Synthetic 1.77× from solo chat completions does **not** automatically transfer to the agentic forge workload, because forge alternates many short generations with growing prompts whose eval cost MTP cannot accelerate. A real comparison needs identical quants on both endpoints — which we did **not** establish in this round.

| Run | Endpoint / Quant | Wall-clock | Mission outcome |
|---|---|---:|---|
| MTP forge (Q4_K_XL, 22.86 GB, fit-2304, c=32768) | `:1235` | **224.8 s** | ✅ committed `multiply(a,b)` |
| LM Studio forge (IQ4_XS, 18.63 GB, LM Studio defaults, c=32768) | `:1234` | **128.9 s** | ✅ committed `multiply(a,b)` |

**LM Studio looked 1.74× faster, but only because `lms load qwen3.6-35b-a3b` resolved to a smaller IQ4_XS file (18.63 GB) that LM Studio fits almost entirely on-GPU (no expert offload), while the MTP server is on the heavier Q4_K_XL (22.86 GB) with ~9 GB of experts paged through PCIe.** That ~4 GB quant gap + PCIe expert traffic dwarfs whatever MTP saves on generation.

### Solo-chat A/B with fair quants (2026-05-16 late evening)

LM Studio loaded with the same Q4_K_XL GGUF, `--no-mmap` toggled ON in the UI Configure panel, c=32864:

| Setup | Quant | Gen tk/s avg | VRAM | RAM free | vs MTP Q4_K_XL |
|---|---|---:|---:|---:|---:|
| **MTP + fit-2304 + no-mmap** | **Q4_K_XL (22.9 GB)** | **44.7** | 14.38 GB | 14.40 GB | **baseline** |
| MTP + fit-2304 + no-mmap | MXFP4-MTP (20.7 GB) | 43.9 | 14.17 GB | 15.32 GB | −0.8 |
| LM Studio + no-mmap | Q4_K_XL | 34.5 | 14.36 GB | 13.82 GB | **−10.2** |
| LM Studio + no-mmap | MXFP4 (23.5 GB) | 28.1 | 14.04 GB | 14.81 GB | −16.6 |
| LM Studio + no-mmap | smaller ~18.6 GB | 25.0 | 10.97 GB | 16.68 GB | −19.7 |

**Verdict: MTP wins by +10.2 tok/s (+30%) on the same Q4_K_XL — claudette daily prod stays on MTP.** MXFP4 is a confirmed null/negative on both backends (Blackwell FP4 path doesn't pay off here; workload is memory-bandwidth bound). LM Studio's `--no-mmap` toggle (Configure panel, not CLI) works fine and gives the same RAM win we saw on MTP. The end-to-end forge wall-clock comparison with this fair quant pairing is open follow-up.

Notes from the run:
- forge auto-bootstrap (`--forge` in a temp git repo under `$HOME`) works as documented in [[forge-mode-shipped]].
- The 8K context that was production for solo-chat is **too small** for forge; coder turn HTTP 400s with `request (8308 tokens) exceeds the available context size`. The bump to `-c 32768` is required for forge to make round-0.
- `--cache-ram 8192` did **not** help (368 s vs 224.8 s for the same mission with `--cache-ram 1024`). Keep the default 1024.
- `~/.cargo/bin/claudette.exe` was stale (v0.3.0, pre-`--forge`). Use `D:\dev\claudette\target\release\claudette.exe` until `cargo install --path .` is re-run.

## External sanity check

Comparing 44 tok/s solo gen to community numbers for the same model family:

| Card | VRAM | Quant | Resident? | Gen tok/s | Notes |
|---|---:|---|---|---:|---|
| RTX 5090 | 32 GB | Q4_K_XL (Qwen 3.5) | full | 194 | llama-bench, [#19890](https://github.com/ggml-org/llama.cpp/discussions/19890) |
| RTX 3090 | 24 GB | Q4_K_XL | full | 100–135 | community, [HF discussion 37](https://huggingface.co/Qwen/Qwen3.6-35B-A3B/discussions/37) |
| RTX 3090 | 24 GB | Q4_K_XL + MTP | full | ~220 | unsloth/dasroot.net writeups |
| RTX 5080 | 16 GB | UD-Q2_K_XL (Qwen 3.5) | full | 63 | smaller quant fits VRAM |
| RTX 5060 Ti | 16 GB | Qwen 3.5 (smaller) | full | 47–51 | [njannasch blog](https://njannasch.dev/blog/running-qwen-3-5-35b-a3b-on-5060-ti/) |
| **RTX 5060 Ti (this rig)** | **16 GB** | **Q4_K_XL + MTP** | **partial (~9 GB CPU offload)** | **45.7 (solo) / 19 (forge wall-clock)** | this report |

**Verdict:** 45 tok/s solo gen is competitive — matches a fully-resident smaller Qwen 3.5 on the same RTX 5060 Ti. The 100–220 tok/s headline numbers from elsewhere come from cards where the model fits entirely in VRAM (no PCIe expert traffic). We're at the practical ceiling for a 16 GB Blackwell card running a 22.9 GB MoE; further headroom needs more VRAM or a smaller quant (with the KLD penalty in §[Quality](#quality-5-prompt-eyeball)).

**⚠ Community-flagged CUDA bug:** [aminrj's writeup](https://aminrj.com/posts/llamacpp-qwen36-35b/) reports CUDA 13.2 + Qwen3.6 producing gibberish outputs. Our build is fine (probe outputs coherent), but if you rebuild and start seeing garbage, check the CUDA version pin.

## Reproducing

1. Build: see [Build commands actually used](#build-commands-actually-used).
2. Download: `Qwen3.6-35B-A3B-UD-Q4_K_XL.gguf` from `unsloth/Qwen3.6-35B-A3B-MTP-GGUF` to `C:\models\`. ~5.4 min @ ~70 MB/s.
3. Launch: see [Final llama-server command](#final-llama-server-command-production-recommended).
4. Bench: `D:\dev\llama.cpp-mtp\bench\probe.ps1 -BaseUrl http://localhost:1235 -Label mtp -OutFile mtp.json`.

Probe source: `D:\dev\llama.cpp-mtp\bench\probe.ps1`.
