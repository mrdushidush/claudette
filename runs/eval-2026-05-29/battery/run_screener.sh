#!/usr/bin/env bash
# SCREEN-10 gate — a cheap, discriminating 10-task screener that decides whether a
# NEW/unknown model earns the full 50-task battery. See SCREENER.md for the design
# and the historical back-test that validates the >=7 gate.
#   usage: bash run_screener.sh <model-key> <identifier> [ctx]
# Steps: unload -> load @ctx (--parallel 1 ALWAYS) -> A1 template/connectivity
# smoke (abort on jinja/HTTP-400) -> run the 10 SCREEN-10 tasks (tag screen-<id>,
# appending) -> score = count of status EXACTLY "PASS" -> print GATE verdict.
set -u
BAT="/d/dev/claudette/runs/eval-2026-05-29/battery"
LMS="/c/Users/david/.lmstudio/bin/lms"
CAPDIR="$HOME/claudette-eval-captures"
mkdir -p "$CAPDIR"

KEY="${1:?model-key required}"
ID="${2:?identifier required}"
CTX="${3:-24576}"

# The screener task set (order = cheap separators first, bigrepo last). Keep in
# sync with SCREENER.md. Excludes I1/I3 on purpose (near-universal fails).
SCREEN_TASKS="A1 A7 C1 C4 D1 F1 H2 H4 I6 I8"

echo "================================================================"
echo "[screener] model-key=$KEY  id=$ID  ctx=$CTX  parallel=1  tasks=[$SCREEN_TASKS]"
echo "================================================================"

# Harness v2.1: BATTERY_SKIP_LMS=1 bypasses lms for an external OpenAI-compat
# server at BATTERY_BASE_URL (see run_model_eval.sh for the contract).
SKIP_LMS="${BATTERY_SKIP_LMS:-0}"
if [ "$SKIP_LMS" = "1" ]; then
  echo "[screener] BATTERY_SKIP_LMS=1 — external server at ${BATTERY_BASE_URL:-http://localhost:1234}; '$ID' must already be served"
else
  "$LMS" unload --all >/dev/null 2>&1
  echo "[screener] loading $KEY @ ${CTX} (parallel 1) ..."
  if ! "$LMS" load "$KEY" -c "$CTX" --parallel 1 --identifier "$ID" -y 2>&1 | tail -1; then
    echo "[screener] LOAD FAILED for $KEY"; exit 5
  fi
  # Confirm CONTEXT / PARALLEL actually took (parallel>1 starves bigrepo tasks).
  "$LMS" ps 2>&1 | grep -iE "IDENTIFIER|$ID|CONTEXT|PARALLEL" || true
fi

export CLAUDETTE_MODEL="$ID" CLAUDETTE_CODER_MODEL="$ID"
export CLAUDETTE_NUM_CTX="$CTX" CLAUDETTE_CODER_NUM_CTX="$CTX"

# Fresh screener scores (filtered runs APPEND, so clear any prior run first).
SCORES="$BAT/SCORES-screen-$ID.tsv"
rm -f "$SCORES"

# Optional reasoning capture (cheap; helps diagnose a template/spiral failure).
# LMS-only — external servers log to their own console.
CAPPID=""
CAP="<none — external server>"
if [ "$SKIP_LMS" != "1" ]; then
  CAP="$CAPDIR/${ID}.screen.stream.log"
  "$LMS" log stream --source model --stats > "$CAP" 2>&1 &
  CAPPID=$!
  sleep 1
fi

# ---- run the 10 tasks (each a single-task filtered run, appending) ----
first=1
for t in $SCREEN_TASKS; do
  BATTERY_TAG="screen-$ID" bash "$BAT/run_battery.sh" "$t" >/dev/null 2>&1
  # After the FIRST task (A1), gate on template/connectivity before wasting time.
  if [ "$first" -eq 1 ]; then
    first=0
    SLOG="$BAT/logs-screen-$ID/A1.log"
    if grep -qiE "jinja template|Unknown StringValue filter|HTTP 400 Bad Request|is not a function|UndefinedValue" "$SLOG" 2>/dev/null; then
      kill "$CAPPID" 2>/dev/null || true
      echo "[screener] TEMPLATE_INCOMPATIBLE: $ID  (LM Studio cannot render this model's chat template)"
      grep -iE 'error"?:' "$SLOG" 2>/dev/null | head -1
      echo "GATE: BLOCKED (template) — record screener row, move on"
      exit 7
    fi
  fi
  # Echo the just-recorded row so progress is visible.
  tail -1 "$SCORES" 2>/dev/null | awk -F'\t' '{printf "  [%s] %s  (%s)\n",$1,$4,$5}'
done

kill "$CAPPID" 2>/dev/null || true

# ---- score = count of status EXACTLY "PASS" (PASS(TIMEOUT) does not count) ----
score=$(awk -F'\t' '$4=="PASS"{n++} END{print n+0}' "$SCORES")
timeouts=$(awk -F'\t' '$4 ~ /TIMEOUT/{n++} END{print n+0}' "$SCORES")
total=$(wc -l < "$SCORES")
echo "================================================================"
echo "[screener] $ID: SCREEN-10 = $score/$total exact-PASS  (timeouts=$timeouts)  capture=$CAP"
if   [ "$score" -ge 7 ]; then verdict="PASS (>=7) -> run the FULL battery"
elif [ "$score" -eq 6 ]; then verdict="BORDERLINE (6) -> re-run the FAILED tasks once (eviction-flake check), then re-judge"
else                          verdict="REJECT (<=5) -> record screener row only, no full battery"
fi
echo "GATE: $verdict"
echo "================================================================"
# Show the failed task ids to make a 6-re-run trivial.
fails=$(awk -F'\t' '$4!="PASS"{printf "%s ",$1}' "$SCORES")
[ -n "$fails" ] && echo "[screener] non-PASS tasks: $fails"
exit 0   # a clean 10/10 must not exit 1 via the && above
