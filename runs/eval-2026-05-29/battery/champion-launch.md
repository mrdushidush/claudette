# Champion launch crib sheet — crowned config 2026-07-11

**Daily driver:** `byteshape/qwen3.6-35b-a3b-mtp` (ShapeLearn IQ3_S-3.06bpw, 13.6 GB,
MTP head bundled) @ **LM Studio**, fully VRAM-resident on the RTX 5060 Ti 16 GB.

## Load command (the one that matters)

```bash
lms load "byteshape/qwen3.6-35b-a3b-mtp" -c 65536 --parallel 1 \
    --speculative-draft-mtp --speculative-draft-max-tokens 2 -y
```

- Verified numbers (2026-07-11): **50/50 + K 8/8 @ 24k** (10.1 min wall) · **49/50 + K 8/8
  @ 64k** · gen **69.8 tok/s @ 64k** / **76.3 @ 24k** · ttft ~2.2 s · battery-wide MTP
  acceptance **95.3%** · VRAM 15.4 GiB @ 64k (resident, ~0.9 GiB headroom) · **zero RAM spill**
  (frees ~7 GB system RAM vs the incumbent's expert offload).
- `--speculative-draft-max-tokens 2` is the measured peak (3 → −4%, 4 → −4%; same shape
  as the May llama-server sweep).
- **Forgot the MTP flags / JIT-loaded by claudette?** Still fine: ~68.7 tok/s NTP-resident
  (MTP in LMS adds only +2–13%; residency is the real win). The per-model default config
  (`~/.lmstudio/.internal/user-concrete-model-default-config/byteshape/…IQ3_S-3.06bpw.gguf.json`)
  already pins **ctx 65536, KV q8_0 K+V, no-mmap, 0 CPU experts, parallel 1, threads 4**,
  so a bare `lms load byteshape/qwen3.6-35b-a3b-mtp -y` or a JIT load inherits the right shape.

## Claudette side

`CLAUDETTE_MODEL=byteshape/qwen3.6-35b-a3b-mtp` (and `CLAUDETTE_CODER_MODEL` if set
separately). Endpoint unchanged: LM Studio `:1234`, `CLAUDETTE_OPENAI_COMPAT=1`.
If loaded with a custom `--identifier`, point `CLAUDETTE_MODEL` at that identifier instead.

## Rollback (keep until the winner has a week of real driving)

Incumbent stays installed: `lms load "qwen3.6-35b-a3b@q3_k_xl" -c 65536 --parallel 1 -y`
(47/50 + K 8/8 @ 24k, 33.8 tok/s). Nothing was deleted in this campaign.

## Gotchas discovered this campaign

- **LMS-MTP gives ~zero net speedup on CPU-offloaded (spilled) quants** — draft acceptance
  is fine (90%+), but the verify batch pays the PCIe tax twice. Only enable-and-expect-gains
  when the quant is fully VRAM-resident.
- For *spilled* quants the speed king remains **llama-server `--fit-target 2304` + MTP**
  (43.1 tok/s on MTP Q4_K_XL; build at `D:\dev\llama.cpp-mtp`, commit `2dff7ff`; needs
  `--jinja` or tool-calling silently degrades).
- IQ-family quants (incl. UD-IQ4_XS) dequant slowly on the CPU-expert path — if a quant
  must spill, prefer Q_K-family; if it's resident, IQ is fine.
- Runtime pin: everything above measured on **LM Studio 0.4.19 + cuda12-avx2 2.24.0**,
  driver 610.62. Template health flips with runtime versions — A1-smoke after upgrades.
