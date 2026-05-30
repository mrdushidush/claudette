#!/usr/bin/env bash
# Daily-driver eval battery runner — claudette v0.8.0 on qwen3.6-35b-a3b@q3_k_xl.
# Per task: fresh-copy fixture -> optional setup hook -> run claudette one-shot
# through the real tool loop -> verify -> record PASS/FAIL + elapsed + recall.
# usage: bash run_battery.sh [id-prefix]   (e.g. "A", "I", "B3" — empty = all)
set -u
BAT="/d/dev/claudette/runs/eval-2026-05-29/battery"
BIN="/d/dev/claudette/target/release/claudette.exe"
export CLAUDETTE_MODEL=qwen3.6-35b-a3b@q3_k_xl
export CLAUDETTE_CODER_MODEL=qwen3.6-35b-a3b@q3_k_xl
export CLAUDETTE_OPENAI_COMPAT=1
export OLLAMA_HOST=http://localhost:1234
export CLAUDETTE_SKIP_OLLAMA_PROBE=1
export CLAUDETTE_NUM_CTX=32768
export CLAUDETTE_CODER_NUM_CTX=32768
export CLAUDETTE_AUTO_APPROVE=1

filter="${1:-}"
SCORES="$BAT/SCORES.tsv"
[ -z "$filter" ] && : > "$SCORES"   # full run resets; filtered run appends

# The "bigrepo" fixture (I1-I8) is a copy of claudette's own src+docs — the
# large-repo-with-conflicting-docs stressor. It's gitignored (it's a dup of the
# repo), so regenerate it on demand from the live tree if missing.
if [ ! -d "$BAT/fixtures/bigrepo/src" ]; then
  echo "[setup] regenerating fixtures/bigrepo from the live repo..."
  REPO="/d/dev/claudette"
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

  work="$BAT/work/$id"; log="$BAT/logs/$id.log"
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

  start=$(date +%s)
  ( cd "$work" && CLAUDETTE_WORKSPACE="$wswin" timeout "$timeout" "$BIN" "$prompt" ) >> "$log" 2>&1
  ec=$?
  elapsed=$(($(date +%s)-start))
  echo "### EXIT=$ec  ELAPSED=${elapsed}s" >> "$log"

  res="$(bash "$BAT/verify/$id.sh" "$work" "$log" 2>&1)"
  status="$(printf '%s\n' "$res" | sed -n 's/^RESULT: \([A-Z]*\).*/\1/p' | head -1)"
  reason="$(printf '%s\n' "$res" | sed -n 's/^RESULT: //p' | head -1)"
  recall="$(printf '%s\n' "$res" | sed -n 's/^RECALL: //p' | head -1)"
  [ -z "$status" ] && status="ERROR"
  [ "$ec" -eq 124 ] && status="${status}(TIMEOUT)"

  printf '%s\t%s\t%s\t%s\t%ds\tEC=%s\trecall=%s\t%s\n' \
    "$id" "$lang" "$type" "$status" "$elapsed" "$ec" "${recall:-na}" "$reason" >> "$SCORES"
  echo "[$id] $status  (${elapsed}s, ec=$ec)  ${recall:+recall=$recall}  ${reason}"
done < "$BAT/manifest.tsv"

echo "================ SUMMARY ================"
p=$(grep -cP '\tPASS\t' "$SCORES"); f=$(grep -cP '\tFAIL' "$SCORES"); t=$(wc -l < "$SCORES")
echo "PASS=$p  FAIL/other=$((t-p))  total=$t"
[ "$t" -gt 0 ] && echo "aggregate: $(awk "BEGIN{printf \"%.1f%%\", 100*$p/$t}")"
