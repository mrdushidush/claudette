#!/usr/bin/env bash
# 100-prompt Claudette regression test with incremental complexity.
# Scores each prompt by checking for expected pattern in the output.
#
# Usage: bash tests/brain100_test.sh [model] [output_dir]
#   Defaults: model=qwen3.5:4b, output_dir=tests/results_brain100

set -uo pipefail

MODEL="${1:-qwen3.5:4b}"
OUTDIR="${2:-tests/results_brain100}"
BINARY="D:/dev/claudette/target/release/claudette.exe"
PROMPTS_FILE="D:/dev/claudette/tests/brain100_prompts.txt"
DELAY=1

mkdir -p "$OUTDIR"

echo "=============================================="
echo "  Claudette Brain Test — 100 Prompts"
echo "  Model: $MODEL"
echo "  Output: $OUTDIR"
echo "=============================================="
echo ""

TOTAL=0
PASS=0
FAIL=0
TIER_COUNTS=(0 0 0 0 0)
TIER_PASS=(0 0 0 0 0)

START_ALL=$(date +%s)

while IFS= read -r line; do
    # Skip comments and blank lines
    [[ "$line" =~ ^#.*$ ]] && continue
    [[ -z "${line// /}" ]] && continue

    # Parse: NUM|||PROMPT|||EXPECTED_PATTERN|||EXPECTED_TOOL
    rest="$line"
    NUM="${rest%%|||*}"
    rest="${rest#*|||}"
    PROMPT="${rest%%|||*}"
    rest="${rest#*|||}"
    EXPECTED="${rest%%|||*}"
    EXPECTED_TOOL="${rest#*|||}"

    [[ -z "$NUM" || -z "$PROMPT" ]] && continue

    TOTAL=$((TOTAL + 1))
    PADDED=$(printf "%03d" "$NUM")

    # Determine tier (1-20=T1, 21-40=T2, etc)
    if [ "$NUM" -le 20 ]; then TIER=0
    elif [ "$NUM" -le 40 ]; then TIER=1
    elif [ "$NUM" -le 60 ]; then TIER=2
    elif [ "$NUM" -le 80 ]; then TIER=3
    else TIER=4
    fi
    TIER_COUNTS[$TIER]=$(( ${TIER_COUNTS[$TIER]} + 1 ))

    # Sleep between prompts to avoid context pollution
    if [ "$TOTAL" -gt 1 ]; then
        sleep "$DELAY"
    fi

    printf "[%3s] %-55s " "$NUM" "${PROMPT:0:55}"

    START=$(date +%s%N)

    # Run single-shot (no --resume), capture stdout+stderr. Redirect stdin
    # to /dev/null so the child binary doesn't steal lines from the prompts
    # file — without this the outer while-read-loop skips ahead and parse
    # misaligns on the next iteration.
    set +e
    OUTPUT=$(CLAUDETTE_MODEL="$MODEL" CLAUDETTE_SKIP_OLLAMA_PROBE=1 timeout 180 "$BINARY" "$PROMPT" < /dev/null 2>&1)
    EXIT_CODE=$?
    set -e

    END=$(date +%s%N)
    ELAPSED_MS=$(( (END - START) / 1000000 ))

    # Save full output
    {
        echo "NUM: $NUM"
        echo "PROMPT: $PROMPT"
        echo "MODEL: $MODEL"
        echo "EXPECTED_PATTERN: $EXPECTED"
        echo "EXPECTED_TOOL: $EXPECTED_TOOL"
        echo "EXIT_CODE: $EXIT_CODE"
        echo "ELAPSED_MS: $ELAPSED_MS"
        echo "---OUTPUT---"
        echo "$OUTPUT"
    } > "$OUTDIR/${PADDED}.txt"

    # Score: exit 0 + non-empty output + expected pattern found (case-insensitive)
    SCORED=0
    if [ "$EXIT_CODE" -eq 0 ] && [ -n "$OUTPUT" ]; then
        if echo "$OUTPUT" | grep -qiE "$EXPECTED" 2>/dev/null; then
            SCORED=1
        fi
    fi

    if [ "$SCORED" -eq 1 ]; then
        PASS=$((PASS + 1))
        TIER_PASS[$TIER]=$(( ${TIER_PASS[$TIER]} + 1 ))
        printf "PASS  (%5dms)\n" "$ELAPSED_MS"
    else
        FAIL=$((FAIL + 1))
        printf "FAIL  (%5dms) [expected: %s]\n" "$ELAPSED_MS" "$EXPECTED"
    fi

done < "$PROMPTS_FILE"

END_ALL=$(date +%s)
TOTAL_SECS=$((END_ALL - START_ALL))

echo ""
echo "=============================================="
echo "  RESULTS: $MODEL"
echo "=============================================="
echo ""
printf "  Total:  %d\n" "$TOTAL"
printf "  Pass:   %d\n" "$PASS"
printf "  Fail:   %d\n" "$FAIL"
printf "  Score:  %d%%\n" "$(( PASS * 100 / TOTAL ))"
echo ""
echo "  Per-tier breakdown:"
TIER_NAMES=("T1: Basic (1-20)" "T2: Params (21-40)" "T3: Multi-step (41-60)" "T4: Edge cases (61-80)" "T5: Complex (81-100)")
for i in 0 1 2 3 4; do
    tc=${TIER_COUNTS[$i]}
    tp=${TIER_PASS[$i]}
    if [ "$tc" -gt 0 ]; then
        pct=$(( tp * 100 / tc ))
    else
        pct=0
    fi
    printf "    %-25s %2d/%2d  (%d%%)\n" "${TIER_NAMES[$i]}" "$tp" "$tc" "$pct"
done
echo ""
printf "  Wall time: %ds\n" "$TOTAL_SECS"
echo "=============================================="

# Write summary file
{
    echo "MODEL: $MODEL"
    echo "TOTAL: $TOTAL"
    echo "PASS: $PASS"
    echo "FAIL: $FAIL"
    echo "SCORE_PCT: $(( PASS * 100 / TOTAL ))"
    echo "WALL_TIME_SECS: $TOTAL_SECS"
    echo ""
    for i in 0 1 2 3 4; do
        echo "TIER_$((i+1)): ${TIER_PASS[$i]}/${TIER_COUNTS[$i]}"
    done
} > "$OUTDIR/summary.txt"

# Write failed prompts for quick review
{
    echo "=== FAILED PROMPTS ==="
    for f in "$OUTDIR"/*.txt; do
        [[ "$(basename "$f")" == "summary.txt" ]] && continue
        [[ "$(basename "$f")" == "failures.txt" ]] && continue
        exitc=$(grep "EXIT_CODE:" "$f" | cut -d' ' -f2)
        exp=$(grep "EXPECTED_PATTERN:" "$f" | cut -d' ' -f2-)
        out=$(sed -n '/---OUTPUT---/,$ p' "$f" | tail -n +2)
        if [ "$exitc" -ne 0 ] || [ -z "$out" ] || ! echo "$out" | grep -qiE "$exp" 2>/dev/null; then
            echo ""
            grep "NUM:" "$f"
            grep "PROMPT:" "$f"
            grep "EXPECTED_PATTERN:" "$f"
            echo "EXIT_CODE: $exitc"
            echo "OUTPUT_PREVIEW: $(echo "$out" | head -3)"
            echo "---"
        fi
    done
} > "$OUTDIR/failures.txt"
