#!/usr/bin/env bash
# Run only the 10 prompts that gpt-oss-20b failed with hallucinated-sandbox
# refusals on the unfixed prompt. Pass model + variant tag; output goes to
# tests/results_sandbox10_<variant>/ and a summary printed at the end.
#
# Usage: bash tests/brain100_sandbox10.sh <variant_tag>
#   (model is read from CLAUDETTE_MODEL env)

set -uo pipefail

VARIANT="${1:-default}"
MODEL="${CLAUDETTE_MODEL:?set CLAUDETTE_MODEL}"
BINARY="D:/dev/claudette/target/release/claudette.exe"
PROMPTS_FILE="D:/dev/claudette/tests/brain100_prompts.txt"
OUTDIR="D:/dev/claudette/tests/results_sandbox10_${VARIANT}"
TARGETS=(13 14 23 24 25 41 48 52 73 80)

mkdir -p "$OUTDIR"
echo "=========================================="
echo "  Sandbox-10 — variant=$VARIANT model=$MODEL"
echo "=========================================="

PASS=0
FAIL=0
START_ALL=$(date +%s)

for NUM in "${TARGETS[@]}"; do
    LINE=$(grep -E "^${NUM}\|\|\|" "$PROMPTS_FILE" | head -1)
    [[ -z "$LINE" ]] && { echo "[$NUM] PROMPT NOT FOUND — skipping"; continue; }

    rest="$LINE"
    rest="${rest#*|||}"
    PROMPT="${rest%%|||*}"
    rest="${rest#*|||}"
    EXPECTED="${rest%%|||*}"

    PADDED=$(printf "%03d" "$NUM")
    printf "[%3s] %-55s " "$NUM" "${PROMPT:0:55}"

    START=$(date +%s%N)
    OUTPUT=$(CLAUDETTE_MODEL="$MODEL" CLAUDETTE_SKIP_OLLAMA_PROBE=1 timeout 90 "$BINARY" "$PROMPT" < /dev/null 2>&1)
    EXIT_CODE=$?
    END=$(date +%s%N)
    ELAPSED_MS=$(( (END - START) / 1000000 ))

    {
        echo "NUM: $NUM"
        echo "VARIANT: $VARIANT"
        echo "PROMPT: $PROMPT"
        echo "EXPECTED: $EXPECTED"
        echo "EXIT_CODE: $EXIT_CODE"
        echo "ELAPSED_MS: $ELAPSED_MS"
        echo "---OUTPUT---"
        echo "$OUTPUT"
    } > "$OUTDIR/${PADDED}.txt"

    if [ "$EXIT_CODE" -eq 0 ] && echo "$OUTPUT" | grep -qiE "$EXPECTED" 2>/dev/null; then
        PASS=$((PASS + 1))
        printf "PASS  (%5dms)\n" "$ELAPSED_MS"
    else
        FAIL=$((FAIL + 1))
        printf "FAIL  (%5dms)\n" "$ELAPSED_MS"
    fi
done

END_ALL=$(date +%s)
WALL=$((END_ALL - START_ALL))

echo ""
echo "=========================================="
echo "  variant=$VARIANT  →  ${PASS}/10 passed in ${WALL}s"
echo "=========================================="
echo "$VARIANT,$PASS,$FAIL,$WALL" >> D:/dev/claudette/tests/sandbox10_scoreboard.csv
