#!/usr/bin/env bash
# Daily-driver eval battery runner — claudette v0.8.0 on qwen3.6-35b-a3b@q3_k_xl.
# Per task: fresh-copy fixture -> optional setup hook -> run claudette one-shot
# through the real tool loop -> verify -> record PASS/FAIL + elapsed + recall.
# usage: bash run_battery.sh [id-prefix]   (e.g. "A", "I", "B3" — empty = all)
set -u
# Battery home = this script's own directory, so the harness runs from any
# clone on any box. Override with BATTERY_HOME only if you have relocated the
# corpus away from the scripts.
BAT="${BATTERY_HOME:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
# Binary: default to the cargo-installed claudette on PATH. The freshly-built
# target/release exe is blocked by Windows Application Control (WDAC) on this box
# and pops a per-launch dialog; the PATH binary is already approved and is the
# same v0.8.1 (it's what users `cargo install`). Override with CLAUDETTE_BIN.
# NOTE: we deliberately do NOT probe target/release here — probing it triggers
# the WDAC popup. Set CLAUDETTE_BIN explicitly if you want a specific build.
BIN="${CLAUDETTE_BIN:-$(command -v claudette)}"
# Model + context are overridable from the environment so the SAME battery can be
# run across many models for a comparison. Defaults reproduce the q3 baseline.
: "${CLAUDETTE_MODEL:=qwen3.6-35b-a3b@q3_k_xl}"
: "${CLAUDETTE_CODER_MODEL:=$CLAUDETTE_MODEL}"
: "${CLAUDETTE_NUM_CTX:=32768}"
: "${CLAUDETTE_CODER_NUM_CTX:=$CLAUDETTE_NUM_CTX}"
export CLAUDETTE_MODEL CLAUDETTE_CODER_MODEL CLAUDETTE_NUM_CTX CLAUDETTE_CODER_NUM_CTX
export CLAUDETTE_OPENAI_COMPAT=1
# Server-agnostic (harness v2.1): BATTERY_BASE_URL points claudette at any
# OpenAI-compat server (external llama-server, LMS on another port). Default
# preserves the historical LM Studio :1234 path byte-identically.
export OLLAMA_HOST="${BATTERY_BASE_URL:-http://localhost:1234}"
export CLAUDETTE_SKIP_OLLAMA_PROBE=1
export CLAUDETTE_AUTO_APPROVE=1
# BATTERY_TAG suffixes the scores file + logs dir so models don't clobber each other.
TAG="${BATTERY_TAG:-}"
# BATTERY_MANIFEST selects which task list to run (default the frozen core-50).
# The new-features "section K" lives in manifest-ext.tsv and is run as a separate
# pass (tag <id>-ext) so a bare full run stays PASS/50. Keep it OUT of manifest.tsv.
MANIFEST="${BATTERY_MANIFEST:-$BAT/manifest.tsv}"

filter="${1:-}"
SCORES="$BAT/SCORES${TAG:+-$TAG}.tsv"
LOGDIR="$BAT/logs${TAG:+-$TAG}"
mkdir -p "$LOGDIR"
[ -z "$filter" ] && : > "$SCORES"   # full run resets; filtered run appends
echo "[battery] model=$CLAUDETTE_MODEL  ctx=$CLAUDETTE_NUM_CTX  tag='${TAG:-<none>}'  manifest=$(basename "$MANIFEST")  scores=$(basename "$SCORES")  base=$OLLAMA_HOST"

# The "bigrepo" fixture (I1-I8) is a copy of claudette's own src+docs — the
# large-repo-with-conflicting-docs stressor. It's gitignored (it's a dup of the
# repo), so regenerate it on demand from the live tree if missing.
if [ ! -d "$BAT/fixtures/bigrepo/src" ]; then
  echo "[setup] regenerating fixtures/bigrepo from the live repo..."
  # The battery lives at <repo>/runs/eval-2026-05-29/battery, so the repo root
  # is three levels up. Verified before use: a relocated corpus (or a partial
  # clone) must fail loudly here rather than silently build a truncated fixture
  # that would change what I1-I8 measure.
  REPO="${BATTERY_REPO:-$(cd "$BAT/../../.." && pwd)}"
  if [ ! -d "$REPO/crates/claudette/src" ]; then
    echo "[setup] ERROR: cannot locate the claudette source tree from $BAT" >&2
    echo "[setup]   looked for: $REPO/crates/claudette/src" >&2
    echo "[setup]   set BATTERY_REPO=/path/to/claudette and re-run." >&2
    exit 1
  fi
  mkdir -p "$BAT/fixtures/bigrepo"
  cp -r "$REPO/crates/claudette/src" "$BAT/fixtures/bigrepo/src"
  cp -r "$REPO/docs" "$BAT/fixtures/bigrepo/docs"
  cp "$REPO/README.md" "$REPO/PRIVACY.md" "$BAT/fixtures/bigrepo/"
  cp "$REPO/crates/claudette/Cargo.toml" "$BAT/fixtures/bigrepo/Cargo.toml"
fi

while IFS=$'\t' read -r id lang type fixture timeout; do
  [ -z "${id:-}" ] && continue
  case "$id" in \#*) continue;; esac
  if [ -n "$filter" ]; then case "$id" in $filter*) ;; *) continue;; esac; fi

  work="$BAT/work/$id"; log="$LOGDIR/$id.log"
  rm -rf "$work"
  cp -r "$BAT/fixtures/$fixture" "$work"
  [ -f "$BAT/setup/$id.sh" ] && bash "$BAT/setup/$id.sh" "$work" >/dev/null 2>&1
  prompt="$(cat "$BAT/prompts/$id.txt")"
  wswin="$(cygpath -m "$work")"

  {
    echo "### $id  [$lang / $type]  fixture=$fixture  timeout=${timeout}s"
    echo "### PROMPT:"
    echo "$prompt"
    echo "### ---- claudette output ----"
  } > "$log"

  # Run claudette under a hard timeout. EC=124 on a LOCAL model is usually an
  # LM Studio eviction / JIT-load flake — the model got unloaded and the cold
  # reload blew the time budget — NOT a code regression (see the unload
  # investigation note). Retry ONCE on a timeout: the first attempt warms the
  # model, so a genuine flake passes on the retry while a real spiral times out
  # again and keeps its (TIMEOUT) stamp, no longer masking real regressions.
  # Tune with BATTERY_TIMEOUT_RETRIES (default 1; 0 disables).
  retries="${BATTERY_TIMEOUT_RETRIES:-1}"
  attempt=0
  while : ; do
    start=$(date +%s)
    # stdin MUST be /dev/null: this loop reads the manifest on the shell's stdin,
    # and claudette inherits that fd. A model that spirals into a stdin-reading
    # path (seen with qwen3-coder-30b @ iter=20) would otherwise swallow the
    # remaining manifest lines and silently truncate the battery mid-run.
    ( cd "$work" && CLAUDETTE_WORKSPACE="$wswin" timeout "$timeout" "$BIN" "$prompt" ) < /dev/null >> "$log" 2>&1
    ec=$?
    elapsed=$(($(date +%s)-start))
    { [ "$ec" -ne 124 ] || [ "$attempt" -ge "$retries" ]; } && break
    attempt=$((attempt+1))
    echo "### TIMEOUT (ec=124) — retry $attempt/$retries (likely model eviction; warming)" >> "$log"
    # Fresh work tree for the retry so a partial mutation from the timed-out
    # attempt doesn't taint the verifier.
    rm -rf "$work"; cp -r "$BAT/fixtures/$fixture" "$work"
    [ -f "$BAT/setup/$id.sh" ] && bash "$BAT/setup/$id.sh" "$work" >/dev/null 2>&1
  done
  suffix=""
  [ "$attempt" -gt 0 ] && suffix="  (after $attempt timeout-retr$([ "$attempt" -eq 1 ] && echo y || echo ies))"
  echo "### EXIT=$ec  ELAPSED=${elapsed}s$suffix" >> "$log"

  res="$(bash "$BAT/verify/$id.sh" "$work" "$log" 2>&1)"
  status="$(printf '%s\n' "$res" | sed -n 's/^RESULT: \([A-Z]*\).*/\1/p' | head -1)"
  reason="$(printf '%s\n' "$res" | sed -n 's/^RESULT: //p' | head -1)"
  recall="$(printf '%s\n' "$res" | sed -n 's/^RECALL: //p' | head -1)"
  [ -z "$status" ] && status="ERROR"
  [ "$ec" -eq 124 ] && status="${status}(TIMEOUT)"

  printf '%s\t%s\t%s\t%s\t%ds\tEC=%s\trecall=%s\t%s\n' \
    "$id" "$lang" "$type" "$status" "$elapsed" "$ec" "${recall:-na}" "$reason" >> "$SCORES"
  echo "[$id] $status  (${elapsed}s, ec=$ec)  ${recall:+recall=$recall}  ${reason}"
done < "$MANIFEST"

echo "================ SUMMARY ================"
p=$(grep -cP '\tPASS\t' "$SCORES"); f=$(grep -cP '\tFAIL' "$SCORES"); t=$(wc -l < "$SCORES")
echo "PASS=$p  FAIL/other=$((t-p))  total=$t"
[ "$t" -gt 0 ] && echo "aggregate: $(awk "BEGIN{printf \"%.1f%%\", 100*$p/$t}")"
# Total wall-clock. 2026-07-25: `probe_speed.sh` tok/s does NOT predict this — a config
# that probed FASTER (73 vs 70 tok/s) ran 2.5x slower here, because the battery is
# dominated by prompt processing, not generation. Report it on every run.
[ "$t" -gt 0 ] && awk -F'\t' '{gsub(/s/,"",$5); s+=$5} END{printf "wall-clock: %dm%02ds (avg %ds/task)\n", s/60, s%60, s/NR}' "$SCORES"

# Record the runtime config this run was ACTUALLY measured under (KV cache type, ctx,
# parallel, VRAM, claudette build). Q50.md listed these as "held constant" but nothing
# verified them, and the KV type silently flipped to f16 — invalidating comparisons that
# nobody knew were invalid. Appended to RUNMETA.tsv, keyed by tag.
#
# Deliberately at the END, not the start: the model JIT-loads on the first task, so a
# probe at t=0 reports `na`. Best-effort — never fail a completed run over metadata.
if [ -f "$BAT/probe_runtime_config.sh" ]; then
  RUNMETA="$BAT/RUNMETA.tsv"
  if row="$(BATTERY_TAG="$TAG" bash "$BAT/probe_runtime_config.sh" "${TAG:-<none>}" 2>/dev/null)"; then
    printf '%s\n' "$row" >> "$RUNMETA"
    printf '%s' "$row" | awk -F'\t' -v f="$(basename "$RUNMETA")" \
      '{printf "runtime config -> %s: ctx=%s kv=%s/%s parallel=%s vram=%sMiB %s\n", f, $4, $6, $7, $5, $9, $10}'
  else
    echo "[warn] runtime-config probe failed; RUNMETA.tsv not updated for tag '${TAG:-<none>}'"
  fi
fi
