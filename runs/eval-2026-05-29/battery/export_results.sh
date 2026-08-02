#!/usr/bin/env bash
# Regenerate RESULTS-q56.csv — one row per full-56 battery run.
#
# Everything is derived from the committed SCORES-*.tsv and RUNMETA.tsv files;
# nothing is transcribed from notes. Run it from anywhere:
#     bash export_results.sh > RESULTS-q56.csv
#
# Model/quant/size metadata is the one hand-maintained part: GiB figures are
# measured from the GGUF in the LM Studio repo dir, NOT from `lms ls` (which
# reports decimal GB and folds in the mmproj projector).
set -euo pipefail
BAT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$BAT"

# tag|model|params|quant|gib|in_ranking_table
ROWS='
gemma4-qat|google/gemma-4-26b-a4b-qat|26B-A4B|Q4_0|13.45|yes
gemma4-qat-r2|google/gemma-4-26b-a4b-qat|26B-A4B|Q4_0|13.45|yes
gemma4-qat-r3|google/gemma-4-26b-a4b-qat|26B-A4B|Q4_0|13.45|yes
gemma26bit|unsloth/gemma-4-26B-A4B-it|26B-A4B|UD-Q4_K_M|15.78|yes
champ-full56|qwen3.6-35b-a3b-mtp@iq3_s|35B-A3B|IQ3_S 3.06bpw|12.67|yes
champ-full56-v2|qwen3.6-35b-a3b-mtp@iq3_s|35B-A3B|IQ3_S 3.06bpw|12.67|yes
champ-full56-v3|qwen3.6-35b-a3b-mtp@iq3_s|35B-A3B|IQ3_S 3.06bpw|12.67|yes
champ-q8-control-r3|qwen3.6-35b-a3b-mtp@iq3_s|35B-A3B|IQ3_S 3.06bpw|12.67|yes
champ-2271-control|qwen3.6-35b-a3b-mtp@iq3_s|35B-A3B|IQ3_S 3.06bpw|12.67|yes
champ-2271-control-r2|qwen3.6-35b-a3b-mtp@iq3_s|35B-A3B|IQ3_S 3.06bpw|12.67|yes
champ-kvfp16-r1|qwen3.6-35b-a3b-mtp@iq3_s|35B-A3B|IQ3_S 3.06bpw|12.67|no-kv-f16-ctx65536
champ-kvfp16-32k-r2|qwen3.6-35b-a3b-mtp@iq3_s|35B-A3B|IQ3_S 3.06bpw|12.67|no-kv-f16
iq4xs|qwen3.6-35b-a3b@iq4_xs|35B-A3B|UD-IQ4_XS 4.25bpw|16.51|yes
mtpgpu3|qwen3.6-35b-a3b-mtp@iq4_xs|35B-A3B|IQ4_XS 3.53bpw|14.59|yes
mtpgpu4|qwen3.6-35b-a3b-mtp-gpu4|35B-A3B|IQ4_XS 3.97bpw|16.43|yes
coder30b|qwen3-coder-30b-a3b-instruct|30B-A3B|UD-Q4_K_XL|16.45|yes
gemma12b|google/gemma-4-12b|12B|Q8_0|11.80|yes
gemma12b-r2|google/gemma-4-12b|12B|Q8_0|11.80|yes
gemmae4b|google/gemma-4-e4b|7.5B|Q4_K_M|4.97|yes
gemmae4b-r2|google/gemma-4-e4b|7.5B|Q4_K_M|4.97|yes
gemmae4b-r3|google/gemma-4-e4b|7.5B|Q4_K_M|4.97|yes
northmini|north-mini-code-1.0|30B-A3B|UD-Q3_K_M|13.24|yes
gemma12bqat|google/gemma-4-12b-qat|12B|Q4_0|6.50|yes
devstral-iq4xs|devstral-small-2-24b-instruct-2512|24B|IQ4_XS|11.90|yes
devstral-iq4xs-r2|devstral-small-2-24b-instruct-2512|24B|IQ4_XS|11.90|yes
devstral-iq4xs-r3|devstral-small-2-24b-instruct-2512|24B|IQ4_XS|11.90|yes
gptoss20b|openai/gpt-oss-20b|20B-A3.6B|MXFP4|11.28|yes
gptoss20b-r2|openai/gpt-oss-20b|20B-A3.6B|MXFP4|11.28|yes
gptoss20b-r3|openai/gpt-oss-20b|20B-A3.6B|MXFP4|11.28|yes
qwen354b|qwen3.5-4b|4B|UD-Q8_K_XL|5.54|yes
qwen354b-r2|qwen3.5-4b|4B|UD-Q8_K_XL|5.54|yes
qwen354b-r3|qwen3.5-4b|4B|UD-Q8_K_XL|5.54|yes
gemmae2b|google/gemma-4-e2b|4.6B|Q4_K_M|3.19|yes
gemmae2b-r2|google/gemma-4-e2b|4.6B|Q4_K_M|3.19|yes
gemmae2b-r3|google/gemma-4-e2b|4.6B|Q4_K_M|3.19|yes
granite8b|unsloth/granite-4.1-8b|8B|Q8_0|8.70|yes
'

echo "model,params,quant,gib,run_tag,date,score,tasks,wall_clock_s,engine,vram_mib,ctx,kv,parallel,in_ranking_table,failed_tasks"

echo "$ROWS" | while IFS='|' read -r tag model params quant gib intable; do
    [ -n "$tag" ] || continue
    f="SCORES-q50-$tag.tsv"
    # Refuse, don't warn-and-continue. Skipping produced a silently SHORT
    # CSV (31 rows instead of 36) while still exiting 0 — the published
    # file then could not be reproduced from a clean checkout (roast
    # BENCH-04). A missing scores file is a broken replication package.
    if [ ! -f "$f" ]; then
        echo "export_results.sh: FATAL — $f is missing; the replication" >&2
        echo "  package is incomplete. Every row in ROWS needs its" >&2
        echo "  SCORES-q50-<tag>.tsv committed. Refusing to emit a" >&2
        echo "  partial CSV." >&2
        exit 1
    fi

    tasks=$(wc -l < "$f" | tr -d ' ')
    score=$(grep -cP '\tPASS\t' "$f" || true)
    secs=$(awk -F'\t' '{s=$5; gsub(/s$/,"",s); t+=s} END{print t+0}' "$f")
    # `~ /^FAIL/`, not `== "FAIL"` — the status column also carries
    # FAIL(TIMEOUT), and an exact match silently dropped those, leaving
    # three rows summing to 55 instead of 56 (roast BENCH-04).
    fails=$(awk -F'\t' '$4 ~ /^FAIL/{printf "%s ", $1}' "$f" | sed 's/ $//')

    # RUNMETA columns: 1 date, 2 tag, 3 model_key, 4 ctx, 5 parallel, 6 kCache,
    # 7 vCache, 8 host, 9 vram_mib, ..., 16 engine
    meta=$(awk -F'\t' -v t="q50-$tag" '$2==t {print; exit}' RUNMETA.tsv)
    if [ -n "$meta" ]; then
        date=$(echo "$meta" | cut -f1)
        ctx=$(echo "$meta"  | cut -f4)
        par=$(echo "$meta"  | cut -f5)
        kv=$(echo "$meta"   | cut -f6)
        vram=$(echo "$meta" | cut -f9)
        eng=$(echo "$meta"  | cut -f16 | sed 's/^llama\.cpp-win-x86_64-nvidia-cuda12-avx2@//')
    else
        date=""; ctx=""; par=""; kv=""; vram=""; eng="not-recorded"
    fi

    printf '%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,%s,"%s"\n' \
        "$model" "$params" "$quant" "$gib" "q50-$tag" "$date" \
        "$score" "$tasks" "$secs" "$eng" "$vram" "$ctx" "$kv" "$par" \
        "$intable" "$fails"
done
