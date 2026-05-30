#!/usr/bin/env bash
# Summarize SCORES.tsv: aggregate + by-type + by-lang + failure list.
set -u
BAT="/d/dev/claudette/runs/eval-2026-05-29/battery"
# Accepts an explicit scores file as $1, else honors BATTERY_TAG, else the default.
S="${1:-$BAT/SCORES${BATTERY_TAG:+-$BATTERY_TAG}.tsv}"
[ -f "$S" ] || { echo "no scores file: $S"; exit 0; }

total=$(wc -l < "$S")
pass=$(awk -F'\t' '$4=="PASS"{n++}END{print n+0}' "$S")
echo "==================== AGGREGATE ===================="
printf 'PASS %d / %d  = %.1f%%\n' "$pass" "$total" "$(awk "BEGIN{print 100*$pass/$total}")"
echo
echo "==================== BY TYPE ======================"
awk -F'\t' '{t=$3; tot[t]++; if($4=="PASS")p[t]++}
  END{for(k in tot) printf "%-16s %d/%d\n", k, p[k]+0, tot[k]}' "$S" | sort
echo
echo "==================== BY LANG ======================"
awk -F'\t' '{l=$2; tot[l]++; if($4=="PASS")p[l]++}
  END{for(k in tot) printf "%-10s %d/%d\n", k, p[k]+0, tot[k]}' "$S" | sort
echo
echo "==================== NON-PASS ====================="
awk -F'\t' '$4!="PASS"{printf "%-4s %-8s %-16s %-14s %s  %s\n",$1,$2,$3,$4,$5,$8}' "$S"
echo
echo "==================== TIMING ======================="
awk -F'\t' '{e=$5; gsub(/s/,"",e); e+=0; s+=e; if(e>mx){mx=e;mxid=$1}} END{printf "total model wall: %ds (%.1f min)  slowest: %s @ %ds\n", s, s/60, mxid, mx}' "$S"
