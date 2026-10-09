#!/usr/bin/env bash
# Record the runtime config a battery row was ACTUALLY measured under.
#
# Why this exists (2026-07-23): Q50.md has always listed "ctx 32768, KV q8_0,
# parallel 1" under "Held constant for every row (comparability)" — but nothing
# ever *recorded* or *verified* it. The KV cache type then silently changed to
# f16 on this box, and we compared rows across two different KV settings without
# knowing. That invalidated the comparison, not because the docs were wrong, but
# because a held-constant nobody measures is a held-constant nobody holds.
#
# The KV cache type is the nasty one: it is STICKY, GLOBAL, and NOT an `lms load`
# flag. It lives in LM Studio's per-model default-config JSON and survives
# unloads, so a bare `lms load` — or a claudette JIT load — silently inherits
# whatever it was last set to. `lms ps --json` does NOT report it.
#
# It is recoverable, though: `lms ps --json` gives the model's `path`, and the
# config file is that same path under the user-concrete-model-default-config
# tree. So we derive it generically rather than hardcoding the champion.
#
# usage: bash probe_runtime_config.sh [tag]        # TSV row on stdout
# Writes a header first if the destination file does not exist yet.
#
# Everything degrades to `na` rather than failing: the harness is backend-
# agnostic (BATTERY_BASE_URL can point at llama-server or a box with no `lms`
# at all), and a missing probe must never take down a battery run.
set -u

TAG="${1:-${BATTERY_TAG:-<none>}}"

# LM Studio home. Override for a non-default install.
LMS_HOME="${LMS_HOME:-$HOME/.lmstudio}"
CFG_ROOT="$LMS_HOME/.internal/user-concrete-model-default-config"

na() { printf 'na'; }

# ---- what is loaded right now -------------------------------------------------
PS_JSON=''
if command -v lms >/dev/null 2>&1; then
  PS_JSON="$(lms ps --json 2>/dev/null || true)"
fi

# Pull the first LLM entry's fields. Python is already a hard dependency of the
# battery (the Q13-Q25 wave runs pytest), so leaning on it costs nothing new,
# and `jq` is NOT present on this box.
read_ps() {
  [ -n "$PS_JSON" ] || { na; return; }
  printf '%s' "$PS_JSON" | python -c '
import json, sys
try:
    d = json.load(sys.stdin)
except Exception:
    print("na"); sys.exit()
rows = [m for m in d if m.get("type") == "llm"] if isinstance(d, list) else []
if not rows:
    print("na"); sys.exit()
v = rows[0].get(sys.argv[1])
print("na" if v is None else v)
' "$1" 2>/dev/null || na
}

IDENT="$(read_ps identifier)"
MPATH="$(read_ps path)"
CTX="$(read_ps contextLength)"
PAR="$(read_ps parallel)"

# ---- the KV cache type, dug out of the sticky default-config ------------------
# `checked: false` means the override is present but DISABLED, which is not the
# same as f16 — report it distinctly so a row is never silently mislabelled.
read_kv() {
  local which="$1" file="$CFG_ROOT/$MPATH.json"
  { [ "$MPATH" = "na" ] || [ ! -f "$file" ]; } && { na; return; }
  python -c '
import json, sys
key = "llm.load.llama.%sCacheQuantizationType" % sys.argv[2]
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        cfg = json.load(fh)
except Exception:
    print("na"); sys.exit()
for f in cfg.get("load", {}).get("fields", []):
    if f.get("key") == key:
        v = f.get("value")
        if isinstance(v, dict):
            print(v.get("value", "na") if v.get("checked") else "off(default)")
        else:
            print(v if v is not None else "na")
        sys.exit()
print("unset(default)")
' "$file" "$which" 2>/dev/null || na
}

KV_K="$(read_kv k)"
KV_V="$(read_kv v)"

# ---- the GPU/CPU expert split (added 2026-07-25) -------------------------------
# Same class of hazard as KV: it lives in the sticky per-model config, is not an
# `lms load` flag, and `lms ps --json` does not report it. It moved mid-session on
# MTP-GPU-4 (LM Studio's auto-split left 2.4 GB of VRAM unused; forcing more
# experts onto the GPU took the probe 21.5 -> 37.4 tok/s and A1 67 s -> 30 s).
# VRAM alone only records it indirectly, so read the knobs themselves.
# "unset(default)" means LM Studio auto-split for the hardware — which is a real,
# reproducible-only-on-this-box setting, not a neutral one.
read_cfg_plain() {
  local key="$1" file="$CFG_ROOT/$MPATH.json"
  { [ "$MPATH" = "na" ] || [ ! -f "$file" ]; } && { na; return; }
  python -c '
import json, sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        cfg = json.load(fh)
except Exception:
    print("na"); sys.exit()
for f in cfg.get("load", {}).get("fields", []):
    if f.get("key") == sys.argv[2]:
        v = f.get("value")
        if isinstance(v, dict):
            print(v.get("value", "na") if v.get("checked") else "off(default)")
        else:
            print("na" if v is None else v)
        sys.exit()
print("unset(default)")
' "$file" "$key" 2>/dev/null || na
}

CPU_EXP="$(read_cfg_plain llm.load.numCpuExpertLayersRatio)"
OFFLOAD="$(read_cfg_plain llm.load.llama.acceleration.offloadRatio)"

# ---- VRAM, for spill detection ------------------------------------------------
VRAM='na'
if command -v nvidia-smi >/dev/null 2>&1; then
  VRAM="$(nvidia-smi --query-gpu=memory.used --format=csv,noheader,nounits 2>/dev/null | head -1 | tr -d ' ')"
  [ -n "$VRAM" ] || VRAM='na'
fi

# ---- the OTHER uncontrolled variable: which claudette actually ran ------------
# run_battery.sh resolves `command -v claudette` by default, so a `cargo install`
# between two runs silently swaps the agent under the battery. The version string
# alone is not enough — it stays "0.17.0" across every dev rebuild of the same
# release — so record the binary's mtime as a cheap build fingerprint too.
BIN_PATH="${CLAUDETTE_BIN:-$(command -v claudette 2>/dev/null || true)}"
BIN_VER='na'; BIN_MTIME='na'
if [ -n "$BIN_PATH" ] && [ -x "$BIN_PATH" ]; then
  BIN_VER="$("$BIN_PATH" --version 2>/dev/null | head -1 | tr -d '\r' || true)"
  [ -n "$BIN_VER" ] || BIN_VER='na'
  # GNU stat and BSD/macOS stat disagree on flags; try both, then give up.
  BIN_MTIME="$(stat -c %y "$BIN_PATH" 2>/dev/null | cut -d. -f1 \
    || stat -f %Sm -t '%Y-%m-%d %H:%M:%S' "$BIN_PATH" 2>/dev/null || true)"
  [ -n "$BIN_MTIME" ] || BIN_MTIME='na'
fi

# ---- and the third uncontrolled variable: which INFERENCE ENGINE actually ran --
# LM Studio ships llama.cpp as a versioned runtime extension and silently installs
# newer ones; the selected engine is what a load actually executes. It flips chat
# templates in BOTH directions (gemma-4 failed the A1 gate on 2.24.0 and passed on
# 2.25.2) and changes kernels, so it is a held constant every bit as much as ctx or
# KV precision. It went unrecorded until 2026-07-26, when `lms runtime ls` revealed
# 2.27.1 had been installed at 01:21 *between* gemma runs r1 and r2 — nobody asked
# for it. Recorded from here on; historical rows carry `na`.
ENGINE='na'
if command -v lms >/dev/null 2>&1; then
  # The selected engine is the row flagged with a check mark; strip it and take
  # the engine id. Tolerates the column shifting, hence the field-independent grep.
  ENGINE="$(lms runtime ls 2>/dev/null | grep -F '✓' | head -1 \
    | tr -d '\r' | awk '{print $1}' || true)"
  [ -n "$ENGINE" ] || ENGINE='na'
fi

# `measured` distinguishes these rows from the hand-reconstructed pre-2026-07-25
# ones in RUNMETA.tsv, which are labelled `inferred`. Same columns either way so
# a probe row appends straight onto the table.
# cpu_expert_ratio/offload_ratio/engine are APPENDED after model_path so every
# existing column index is unchanged and the awk one-liners in the dossier still
# work. Rows written before 2026-07-25 carry `na` in the first two; rows before
# 2026-07-26 carry `na` in engine.
printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
  "$(date +%Y-%m-%d)" "$TAG" "$IDENT" "$CTX" "$PAR" "$KV_K" "$KV_V" \
  "${BATTERY_BASE_URL:-http://localhost:1234}" "$VRAM" "$BIN_VER" "$BIN_MTIME" \
  'measured' "$MPATH" "$CPU_EXP" "$OFFLOAD" "$ENGINE"
