#!/usr/bin/env bash
# Run brain100_test.sh against multiple LM Studio models for brain shopping.
# For each model: lms unload, lms load at 16K context, run brain100 with
# CLAUDETTE_OPENAI_COMPAT=1 pointing at LM Studio's /v1 endpoint.
# Saves results to tests/results_brain100_<model_safe_name>/ per model.
#
# Usage: bash tests/brain100_lmstudio_shopping.sh
# Wall time: ~15-20 min/model × N models. Background it.

set -uo pipefail

MODELS=(
    "openai/gpt-oss-20b"
    "unsloth/gpt-oss-20b"
    "google/gemma-4-26b-a4b"
)

cd "$(dirname "$0")/.."

# LM Studio compat-mode env: applied for every brain100_test.sh invocation.
# CLAUDETTE_MAX_TOOLS empty = no cap (lets the model see the full registry,
# which is what claudette uses in production with Ollama).
export OLLAMA_HOST="http://localhost:1234"
export CLAUDETTE_OPENAI_COMPAT=1
export CLAUDETTE_FALLBACK_BRAIN_MODEL=
export CLAUDETTE_MAX_TOOLS=
export CLAUDETTE_SKIP_OLLAMA_PROBE=1
# Without this the file-ops tools refuse reads under D:/dev/claudette since
# the cwd is not under $HOME on Windows. Causes models to relay refusals to
# the user, depressing scores by ~20 pts vs the Ollama baseline (the prior
# Ollama runs were taken with this var set; the LM Studio wrapper omitted
# it, producing a misdiagnosed "compat-layer gap"). Use absolute repo root.
export CLAUDETTE_WORKSPACE="$(pwd)"

START_ALL=$(date +%s)

for model in "${MODELS[@]}"; do
    safe_name=$(echo "$model" | tr '/' '_')
    outdir="tests/results_brain100_${safe_name}"

    echo ""
    echo "############################################################"
    echo "## $model -> $outdir"
    echo "############################################################"

    # Swap LM Studio to the candidate model. Skip the lms output spinner
    # spam by tail-only.
    lms unload --all 2>&1 | tail -1
    if ! lms load "$model" --context-length 16384 --gpu max -y 2>&1 | tail -2; then
        echo "FAILED to load $model — skipping" | tee -a tests/brain100_shopping.log
        continue
    fi

    # Run the bench. The brain100_test.sh script reads CLAUDETTE_MODEL via
    # the env it sets per-prompt, but we also pass it as $1 for the script's
    # own logging. Output goes to outdir.
    bash tests/brain100_test.sh "$model" "$outdir"
done

END_ALL=$(date +%s)
WALL_ALL=$((END_ALL - START_ALL))

echo ""
echo "############################################################"
echo "## ALL MODELS DONE — wall time ${WALL_ALL}s"
echo "############################################################"

# Aggregate per-model summaries into a single scoreboard.
{
    echo "# Brain100 LM Studio Shopping Results"
    echo ""
    echo "Wall time: ${WALL_ALL}s"
    echo ""
    echo "| Model | Score | Pass | T1 | T2 | T3 | T4 | T5 | Time |"
    echo "|-------|-------|------|----|----|----|----|----|------|"
    for model in "${MODELS[@]}"; do
        safe_name=$(echo "$model" | tr '/' '_')
        outdir="tests/results_brain100_${safe_name}"
        sf="$outdir/summary.txt"
        if [ ! -f "$sf" ]; then
            echo "| $model | (no summary) | - | - | - | - | - | - | - |"
            continue
        fi
        score=$(grep -E "^SCORE_PCT:" "$sf" | awk '{print $2}')
        pass=$(grep -E "^PASS:" "$sf" | awk '{print $2}')
        total=$(grep -E "^TOTAL:" "$sf" | awk '{print $2}')
        wall=$(grep -E "^WALL_TIME_SECS:" "$sf" | awk '{print $2}')
        t1=$(grep -E "^TIER_1:" "$sf" | awk '{print $2}')
        t2=$(grep -E "^TIER_2:" "$sf" | awk '{print $2}')
        t3=$(grep -E "^TIER_3:" "$sf" | awk '{print $2}')
        t4=$(grep -E "^TIER_4:" "$sf" | awk '{print $2}')
        t5=$(grep -E "^TIER_5:" "$sf" | awk '{print $2}')
        echo "| $model | ${score}% | ${pass}/${total} | $t1 | $t2 | $t3 | $t4 | $t5 | ${wall}s |"
    done
} | tee tests/brain100_shopping_scoreboard.md

echo ""
echo "Scoreboard: tests/brain100_shopping_scoreboard.md"
