#!/usr/bin/env bash
# Per-model eval driver for the multi-model comparison.
#   usage: bash run_model_eval.sh <model-key> <identifier> [ctx] [filter]
# Steps: unload -> load@ctx (--parallel 1 ALWAYS) -> A1 smoke (abort if the
# model's chat template is incompatible / 400s) -> start reasoning capture
# (lms log stream) -> full battery (tagged, non-clobbering) -> stop capture ->
# analyze.
set -u
BAT="/d/dev/claudette/runs/eval-2026-05-29/battery"
LMS="/c/Users/david/.lmstudio/bin/lms"
CAPDIR="$HOME/claudette-eval-captures"
mkdir -p "$CAPDIR"

KEY="${1:?model-key required}"
ID="${2:?identifier required}"
CTX="${3:-24576}"
FILTER="${4:-}"

echo "================================================================"
echo "[driver] model-key=$KEY  id=$ID  ctx=$CTX  parallel=1  $(date '+%F %T')"
echo "================================================================"

"$LMS" unload --all >/dev/null 2>&1
echo "[driver] loading $KEY @ ${CTX} (parallel 1) ..."
if ! "$LMS" load "$KEY" -c "$CTX" --parallel 1 --identifier "$ID" -y 2>&1 | tail -1; then
  echo "[driver] LOAD FAILED for $KEY"; exit 5
fi
# show what actually loaded (CONTEXT + PARALLEL columns must read $CTX / 1)
"$LMS" ps 2>&1 | grep -iE "IDENTIFIER|$ID" || true

# Constant per-run env: export once so child run_battery.sh inherits it.
export CLAUDETTE_MODEL="$ID" CLAUDETTE_CODER_MODEL="$ID"
export CLAUDETTE_NUM_CTX="$CTX" CLAUDETTE_CODER_NUM_CTX="$CTX"

# ---- smoke A1 (template / connectivity gate) ----
echo "[driver] smoke A1 ..."
# run_battery.sh appends for filtered runs, so clear any stale smoke scores first
# (else the gate's NR==1 read picks up a leftover row from a prior session).
rm -f "$BAT/SCORES-smoke-$ID.tsv"
BATTERY_TAG="smoke-$ID" bash "$BAT/run_battery.sh" A1 >/dev/null 2>&1 || true
SLOG="$BAT/logs-smoke-$ID/A1.log"
if grep -qiE "jinja template|Unknown StringValue filter|HTTP 400 Bad Request" "$SLOG" 2>/dev/null; then
  echo "[driver] TEMPLATE_INCOMPATIBLE: $ID  (LM Studio cannot render this model's chat template)"
  grep -iE "error\":" "$SLOG" 2>/dev/null | head -1
  exit 7
fi
SS=$(awk -F'\t' 'NR==1{print $4" "$5}' "$BAT/SCORES-smoke-$ID.tsv" 2>/dev/null)
echo "[driver] smoke A1 = ${SS:-<no result>}  -> proceeding to full battery"

# ---- reasoning capture ----
CAP="$CAPDIR/${ID}.stream.log"
echo "[driver] reasoning capture -> $CAP"
"$LMS" log stream --source model --stats > "$CAP" 2>&1 &
CAPPID=$!
sleep 1

# ---- full battery ----
BATTERY_TAG="$ID" bash "$BAT/run_battery.sh" "$FILTER"
EC=$?

kill "$CAPPID" 2>/dev/null || true

echo "================================================================"
echo "[driver] ANALYZE $ID"
bash "$BAT/analyze.sh" "$BAT/SCORES-$ID.tsv"
echo "[driver] DONE $ID (battery ec=$EC, capture=$CAP)"
