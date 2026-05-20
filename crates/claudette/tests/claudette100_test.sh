#!/usr/bin/env bash
# 100-prompt Claudette full-surface regression test.
# Scores each prompt by checking for expected pattern in the output.
#
# Adapted from brain100_test.sh — same NUM|||PROMPT|||PATTERN|||TOOL format,
# but SKIPS prompts whose TOOL field is a non-runnable sentinel:
#   SLASH_*       — slash commands, only work in REPL/TUI
#   CLI_*         — separate-process invocations like --cto / --faceless
#   TUI_*         — keyboard chords like Ctrl+G
#   VISION        — image attach
#   PASTE_LARGE   — paste handler regression
#
# Usage: bash tests/claudette100_test.sh [model] [output_dir]
#   Defaults: model=$CLAUDETTE_MODEL or qwen3.6-35b-a3b@q4_k_xl
#             output_dir=tests/results_claudette100

set -uo pipefail

MODEL="${1:-${CLAUDETTE_MODEL:-qwen3.6-35b-a3b@q4_k_xl}}"
OUTDIR="${2:-tests/results_claudette100}"
BINARY="D:/dev/claudette/target/release/claudette.exe"
PROMPTS_FILE="${CLAUDETTE100_PROMPTS:-D:/dev/claudette/crates/claudette/tests/claudette100_prompts.txt}"
DELAY=1

mkdir -p "$OUTDIR"

echo "=============================================="
echo "  Claudette 100-Prompt Full-Surface Sweep"
echo "  Model: $MODEL"
echo "  Output: $OUTDIR"
echo "=============================================="
echo ""

TOTAL=0
PASS=0
FAIL=0
SKIP=0
# 10 sections × 10 prompts each
SECTION_COUNTS=(0 0 0 0 0 0 0 0 0 0)
SECTION_PASS=(0 0 0 0 0 0 0 0 0 0)
SECTION_SKIP=(0 0 0 0 0 0 0 0 0 0)

START_ALL=$(date +%s)

is_skip_tool() {
    case "$1" in
        SLASH_*|CLI_*|TUI_*|VISION|PASTE_LARGE) return 0 ;;
        *) return 1 ;;
    esac
}

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

    # Determine section (1-10=S1, 11-20=S2, ..., 91-100=S10)
    SECTION=$(( (NUM - 1) / 10 ))
    [[ $SECTION -lt 0 || $SECTION -gt 9 ]] && SECTION=9
    SECTION_COUNTS[$SECTION]=$(( ${SECTION_COUNTS[$SECTION]} + 1 ))

    PADDED=$(printf "%03d" "$NUM")

    # Skip non-runnable surfaces (slash / chord / image / paste / CLI flag)
    if is_skip_tool "$EXPECTED_TOOL"; then
        SKIP=$((SKIP + 1))
        SECTION_SKIP[$SECTION]=$(( ${SECTION_SKIP[$SECTION]} + 1 ))
        printf "[%3s] %-55s SKIP  (tool=%s)\n" "$NUM" "${PROMPT:0:55}" "$EXPECTED_TOOL"
        continue
    fi

    TOTAL=$((TOTAL + 1))

    # Sleep between prompts to avoid context pollution
    if [ "$TOTAL" -gt 1 ]; then
        sleep "$DELAY"
    fi

    printf "[%3s] %-55s " "$NUM" "${PROMPT:0:55}"

    START=$(date +%s%N)

    set +e
    OUTPUT=$(CLAUDETTE_MODEL="$MODEL" CLAUDETTE_SKIP_OLLAMA_PROBE=1 CLAUDETTE_WORKSPACE="D:/dev/claudette" timeout 180 "$BINARY" "$PROMPT" < /dev/null 2>&1)
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
        SECTION_PASS[$SECTION]=$(( ${SECTION_PASS[$SECTION]} + 1 ))
        printf "PASS  (%5dms)\n" "$ELAPSED_MS"
    else
        FAIL=$((FAIL + 1))
        printf "FAIL  (%5dms) [expected: %.40s]\n" "$ELAPSED_MS" "$EXPECTED"
    fi

done < "$PROMPTS_FILE"

END_ALL=$(date +%s)
TOTAL_SECS=$((END_ALL - START_ALL))

echo ""
echo "=============================================="
echo "  RESULTS: $MODEL"
echo "=============================================="
echo ""
printf "  Runnable: %d   Pass: %d   Fail: %d   Skipped: %d\n" "$TOTAL" "$PASS" "$FAIL" "$SKIP"
if [ "$TOTAL" -gt 0 ]; then
    printf "  Score:    %d%%  (of runnable)\n" "$(( PASS * 100 / TOTAL ))"
fi
echo ""
echo "  Per-section breakdown (S# = 10 prompts):"
SECTION_NAMES=(
    "S1:  Slash dispatcher"
    "S2:  Preset/model/recall"
    "S3:  Brain reasoning"
    "S4:  Secretary (time/notes/todos)"
    "S5:  Filesystem/search/git"
    "S6:  Calendar/Gmail/Schedule"
    "S7:  Web/facts/markets/registry"
    "S8:  GitHub/missions/forge"
    "S9:  Codet/agents/CTO/faceless"
    "S10: Edge cases & safety"
)
for i in 0 1 2 3 4 5 6 7 8 9; do
    tc=${SECTION_COUNTS[$i]}
    tp=${SECTION_PASS[$i]}
    ts=${SECTION_SKIP[$i]}
    runnable=$(( tc - ts ))
    if [ "$runnable" -gt 0 ]; then
        pct=$(( tp * 100 / runnable ))
    else
        pct=0
    fi
    printf "    %-37s %2d/%2d pass  (%d%%, %d skipped)\n" "${SECTION_NAMES[$i]}" "$tp" "$runnable" "$pct" "$ts"
done
echo ""
printf "  Wall time: %ds\n" "$TOTAL_SECS"
echo "=============================================="

# Write summary file
{
    echo "MODEL: $MODEL"
    echo "RUNNABLE: $TOTAL"
    echo "PASS: $PASS"
    echo "FAIL: $FAIL"
    echo "SKIPPED: $SKIP"
    if [ "$TOTAL" -gt 0 ]; then
        echo "SCORE_PCT: $(( PASS * 100 / TOTAL ))"
    fi
    echo "WALL_TIME_SECS: $TOTAL_SECS"
    echo ""
    for i in 0 1 2 3 4 5 6 7 8 9; do
        runnable=$(( ${SECTION_COUNTS[$i]} - ${SECTION_SKIP[$i]} ))
        echo "S$((i+1)): ${SECTION_PASS[$i]}/$runnable pass, ${SECTION_SKIP[$i]} skipped"
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
