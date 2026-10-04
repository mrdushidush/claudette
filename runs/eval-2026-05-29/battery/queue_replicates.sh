#!/usr/bin/env bash
# qwen354b_queue.sh — qwen3.5-4b @ UD-Q8_K_XL, three full-56 runs, fresh load per run.
#
# Same guards as the e2b driver, plus a NO-BENCH-WINDOW guard: this model is ~2x slower
# than e2b, so three runs land near the 16:00 boundary. Rather than risk a kill mid-run
# (which needs the git-bash PID dance and leaves a quarantined partial), the queue simply
# REFUSES to start a run that could cross the window, and exits cleanly having banked
# whatever runs completed.

set -uo pipefail

BAT="D:/dev/claudette/runs/eval-2026-05-29/battery"
MODEL="qwen3.5-4b"
CTX=32768
LOAD_TIMEOUT=600
# Concrete .gguf path — unlike the gemma catalog models, model_path DOES pin the quant here.
WANT_PATH="unsloth/Qwen3.5-4B-GGUF/Qwen3.5-4B-UD-Q8_K_XL.gguf"
# Latest clock time at which a NEW run may start. A run is ~50 min; 15:05 + 50 = 15:55.
LATEST_START="1505"

cd "$BAT" || exit 1

echo "=== qwen3.5-4b queue start $(date '+%F %T') ==="
echo "model=$MODEL ctx=$CTX want_path=$WANT_PATH latest_start=$LATEST_START"

# 2026-08-01: r1 (q50-qwen354b) is BANKED at 31/56 — do not re-run it. r2 was killed
# mid-run on 07-31 and its partial + logs are quarantined; r2 restarts FROM SCRATCH.
for TAG in q50-qwen354b-r2 q50-qwen354b-r3; do
  NOW="$(date '+%H%M')"
  if [ "$NOW" \> "$LATEST_START" ]; then
    echo
    echo "!! STOP: $(date '+%T') is past the $LATEST_START cutoff — not starting $TAG."
    echo "!! A run takes ~50 min and would cross the 16:00 no-bench window."
    echo "!! Completed runs are banked and scored; the remainder is owed."
    break
  fi

  echo
  echo "--- [$TAG] unload+load $(date '+%F %T') ---"
  lms unload --all >/dev/null 2>&1

  timeout "$LOAD_TIMEOUT" lms load "$MODEL" -c "$CTX" -y
  rc=$?
  if [ "$rc" -ne 0 ]; then
    if [ "$rc" -eq 124 ]; then
      echo "!! ABORT: lms load HUNG past ${LOAD_TIMEOUT}s on $TAG."
    else
      echo "!! ABORT: lms load failed rc=$rc on $TAG."
    fi
    exit "$rc"
  fi

  PS_LINE="$(lms ps 2>&1 | grep -F "$MODEL" | head -1)"
  echo "--- [$TAG] resident: $PS_LINE"
  if ! printf '%s' "$PS_LINE" | grep -qE '32768[[:space:]]+1[[:space:]]'; then
    echo "!! ABORT: ctx/parallel drifted (want 32768 / 1). Got: $PS_LINE"
    exit 4
  fi

  echo "--- [$TAG] battery start $(date '+%F %T') ---"
  BATTERY_MANIFEST="$BAT/manifest-q50.tsv" \
  BATTERY_TAG="$TAG" \
  CLAUDETTE_MODEL="$MODEL" \
    bash run_battery.sh

  # The probe appends a RUNMETA row with model_path; assert it names the quant we approved.
  if ! tail -1 "$BAT/RUNMETA.tsv" | grep -qF "$WANT_PATH"; then
    echo "!! WARN: RUNMETA model_path for $TAG does not match $WANT_PATH — CHECK BEFORE PUBLISHING."
  fi
  echo "--- [$TAG] battery done $(date '+%F %T') ---"
done

lms unload --all >/dev/null 2>&1
echo
echo "=== qwen3.5-4b queue complete $(date '+%F %T') ==="
