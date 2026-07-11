#!/usr/bin/env bash
# probe_speed.sh — standard 3-prompt generation-speed probe (harness v2.1).
#   usage: bash probe_speed.sh <model-id> [label]
#   env:   PROBE_BASE_URL   (default http://localhost:1234 — same default as battery)
#          PROBE_REPS       (default 3 — reps per prompt; median reported)
#          PROBE_MAX_TOKENS (default 400)
# Methodology reproduces docs/archive/mtp_benchmark.md (2026-05-16) so numbers are
# comparable to the historical 24.95 (LMS) / 43.16 (MTP) rows: streaming SSE,
# temperature 0, seed 42; gen tok/s counts delta.content + delta.reasoning_content
# chunks, timed FIRST-chunk -> LAST-chunk (excludes prompt-eval / TTFT). If the
# server's final chunk carries llama-server "timings", predicted_per_second is
# reported alongside as server_tps. Appends one row per invocation to
# SPEED-PROBES.tsv (created with a header if missing).
set -u
BAT="/d/dev/claudette/runs/eval-2026-05-29/battery"
MODEL="${1:?model-id required}"
LABEL="${2:-$MODEL}"
BASE="${PROBE_BASE_URL:-http://localhost:1234}"
REPS="${PROBE_REPS:-3}"
MAXTOK="${PROBE_MAX_TOKENS:-400}"
OUT="$BAT/SPEED-PROBES.tsv"

python - "$MODEL" "$LABEL" "$BASE" "$REPS" "$MAXTOK" "$OUT" <<'PYEOF'
import json, statistics, subprocess, sys, time, urllib.request

model, label, base, reps, maxtok, out = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), int(sys.argv[5]), sys.argv[6]

# Agent-shaped fixed prompts (subset of the 2026-05-16 probe set).
PROMPTS = {
    "code_emit_small": "Write a Python function that parses a single CSV line into a list of fields, correctly handling quoted fields containing commas and escaped quotes. Include a short docstring.",
    "explain_medium": "Explain in detail how a hash map handles collisions, covering separate chaining and open addressing, and the performance tradeoffs of each.",
    "refactor_longish": "Refactor this function to be idiomatic and testable, and explain each change:\n\ndef proc(d):\n    r = []\n    for k in d:\n        if d[k] != None:\n            if type(d[k]) == str:\n                r.append(k + '=' + d[k])\n            else:\n                r.append(k + '=' + str(d[k]))\n    return ','.join(r)",
}

def one_call(prompt, max_tokens):
    body = json.dumps({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "stream": True, "temperature": 0, "seed": 42, "max_tokens": max_tokens,
    }).encode()
    req = urllib.request.Request(base.rstrip("/") + "/v1/chat/completions", data=body,
                                 headers={"Content-Type": "application/json"})
    t0 = time.perf_counter(); t_first = None; t_last = None; n = 0; server_tps = None
    with urllib.request.urlopen(req, timeout=300) as resp:
        for raw in resp:
            line = raw.strip()
            if not line.startswith(b"data: "):
                continue
            payload = line[6:]
            if payload == b"[DONE]":
                break
            try:
                obj = json.loads(payload)
            except json.JSONDecodeError:
                continue
            if isinstance(obj.get("timings"), dict):
                server_tps = obj["timings"].get("predicted_per_second", server_tps)
            ch = obj.get("choices") or []
            if not ch:
                continue
            delta = ch[0].get("delta") or {}
            if delta.get("content") or delta.get("reasoning_content"):
                now = time.perf_counter()
                if t_first is None:
                    t_first = now
                t_last = now
                n += 1
    if n < 2:
        return None
    return {"tps": (n - 1) / (t_last - t_first), "ttft": t_first - t0, "ntok": n, "server_tps": server_tps}

# Warmup (cold prompt-cache / JIT load absorber) — not measured.
try:
    one_call("Say OK.", 32)
except Exception as e:
    print(f"[probe] FATAL: warmup call failed against {base}: {e}", file=sys.stderr)
    sys.exit(2)

per_prompt, ttfts, server_tpss = {}, [], []
for name, prompt in PROMPTS.items():
    vals = []
    for i in range(reps):
        r = one_call(prompt, maxtok)
        if r is None:
            print(f"  [{name}] rep{i+1}: <2 tokens streamed — skipped")
            continue
        vals.append(r["tps"]); ttfts.append(r["ttft"])
        if r["server_tps"] is not None:
            server_tpss.append(r["server_tps"])
        print(f"  [{name}] rep{i+1}: {r['tps']:.2f} tok/s  (ttft {r['ttft']:.2f}s, {r['ntok']} tok)")
    if vals:
        per_prompt[name] = statistics.median(vals)

if not per_prompt:
    print("[probe] FATAL: no successful measurements", file=sys.stderr)
    sys.exit(3)

overall = statistics.median(sorted(v for v in per_prompt.values()))
ttft_med = statistics.median(ttfts)
stps = f"{statistics.median(server_tpss):.2f}" if server_tpss else "na"
try:
    vram = subprocess.run(["nvidia-smi", "--query-gpu=memory.used", "--format=csv,noheader,nounits"],
                          capture_output=True, text=True, timeout=10).stdout.strip().splitlines()[0].strip()
except Exception:
    vram = "na"

import datetime, os
header = "date\tlabel\tmodel\tbase_url\tcode_emit\texplain\trefactor\tmedian_tps\tttft_med_s\tserver_tps\tvram_used_mib\n"
row = (f"{datetime.date.today()}\t{label}\t{model}\t{base}\t"
       f"{per_prompt.get('code_emit_small', 0):.2f}\t{per_prompt.get('explain_medium', 0):.2f}\t"
       f"{per_prompt.get('refactor_longish', 0):.2f}\t{overall:.2f}\t{ttft_med:.2f}\t{stps}\t{vram}\n")
new = not os.path.exists(out)
with open(out, "a") as f:
    if new:
        f.write(header)
    f.write(row)
print(f"[probe] {label}: MEDIAN {overall:.2f} tok/s  (ttft {ttft_med:.2f}s, server_tps {stps}, vram {vram} MiB) -> {os.path.basename(out)}")
PYEOF
