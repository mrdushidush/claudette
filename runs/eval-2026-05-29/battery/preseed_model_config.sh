#!/usr/bin/env bash
# preseed_model_config.sh — force a model's LM Studio per-model default config to the
# battery's held-constant load settings BEFORE its first load.
#
# Why this exists: LM Studio stores load settings in a sticky per-model JSON that survives
# unloads and is inherited by a bare `lms load` or a claudette JIT load. It is NOT an
# `lms load` flag and `lms ps --json` does not expose it. A freshly downloaded model gets a
# FRESH config, so KV quantization silently defaults (usually f16) instead of inheriting the
# battery standard. Two campaign nights were spent chasing scores that moved because this
# file changed underneath the runs.
#
# Held constants (Q50.md): ctx 32768, K/V cache q8_0, full GPU offload, 1 parallel session.
# numParallelSessions matters and is easy to miss: a fresh config omits it entirely and LM
# Studio then defaults to 4, which splits the KV allocation across slots. Caught on
# gpt-oss-20b, whose first load probed parallel=4 while every champion run is parallel=1.
# The CPU/GPU expert split is deliberately NOT set here — David calibrates it per model
# after the first load, and probe_runtime_config.sh records what it ended up as.
#
# Usage:
#   bash preseed_model_config.sh <path-to-model-config.json>
#   bash preseed_model_config.sh --find <substring>     # locate a config by fuzzy name
#
# Always prints the before/after values so the change is visible in the run log.
set -euo pipefail

CFG_ROOT="$HOME/.lmstudio/.internal/user-concrete-model-default-config"

if [ $# -eq 0 ]; then
  echo "usage: $0 <config.json> | --find <substring>" >&2
  exit 2
fi

if [ "$1" = "--find" ]; then
  [ $# -ge 2 ] || { echo "--find needs a substring" >&2; exit 2; }
  find "$CFG_ROOT" -name "*.json" | grep -i -- "$2" || {
    echo "no config matches '$2' (model may never have been loaded yet)" >&2
    exit 1
  }
  exit 0
fi

TARGET="$1"
[ -f "$TARGET" ] || { echo "no such config: $TARGET" >&2; exit 1; }

# Back up once per day so a bad edit is always recoverable.
BAK="${TARGET}.bak-preseed-$(date +%Y-%m-%d)"
[ -f "$BAK" ] || cp "$TARGET" "$BAK"

python - "$TARGET" <<'PY'
import json, sys

path = sys.argv[1]
with open(path, encoding="utf-8") as fh:
    cfg = json.load(fh)

WANT = {
    "llm.load.contextLength": 32768,
    "llm.load.llama.kCacheQuantizationType": {"checked": True, "value": "q8_0"},
    "llm.load.llama.vCacheQuantizationType": {"checked": True, "value": "q8_0"},
    "llm.load.llama.acceleration.offloadRatio": 1,
    "llm.load.numParallelSessions": 1,
}

fields = cfg.setdefault("load", {}).setdefault("fields", [])
by_key = {f["key"]: f for f in fields}

for key, want in WANT.items():
    before = by_key[key]["value"] if key in by_key else "<unset>"
    if key in by_key:
        by_key[key]["value"] = want
    else:
        fields.append({"key": key, "value": want})
    flag = "  (unchanged)" if before == want else ""
    print(f"  {key}\n    before: {before}\n    after:  {want}{flag}")

with open(path, "w", encoding="utf-8") as fh:
    json.dump(cfg, fh, indent=1)
PY

echo "preseeded: $TARGET"
echo "backup:    $BAK"
