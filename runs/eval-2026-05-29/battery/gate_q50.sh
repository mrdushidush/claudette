#!/usr/bin/env bash
# gate_q50.sh — authoring gate for the Q50 quality battery.
#
# A task may enter manifest-q50.tsv ONLY if:
#   (1) verify/<id>.sh reports FAIL on the untouched fixture (the bug/gap is real
#       and the hidden tests actually exercise it), AND
#   (2) verify/<id>.sh reports PASS once the reference solution (refsol/<id>/) is
#       overlaid on the fixture (the task is solvable and the tests are fair).
#
# This is the test-first discipline from the campaign goal (Phase 1, decision 2).
# The reference solution lives OUTSIDE the fixture the model sees; the model's
# workspace is only ever a copy of fixtures/<id>.
#
# usage:  bash gate_q50.sh <id> [id2 ...]      (no args = every fixtures/Q* task)
set -u
BAT="/d/dev/claudette/runs/eval-2026-05-29/battery"
DUMMY="$BAT/work-gate/.dummy-transcript"; mkdir -p "$BAT/work-gate"; : > "$DUMMY"

status_of(){ printf '%s\n' "$1" | sed -n 's/^RESULT: \([A-Z]*\).*/\1/p' | head -1; }

gate_one(){
  local id="$1"
  local fx="$BAT/fixtures/$id" vf="$BAT/verify/$id.sh" rs="$BAT/refsol/$id"
  [ -d "$fx" ] || { echo "[$id] GATE-ERROR: no fixture $fx"; return 1; }
  [ -f "$vf" ] || { echo "[$id] GATE-ERROR: no verify $vf"; return 1; }
  [ -d "$rs" ] || { echo "[$id] GATE-ERROR: no refsol $rs"; return 1; }

  # (1) untouched fixture must FAIL
  local w1="$BAT/work-gate/$id-fix"; rm -rf "$w1"; cp -r "$fx" "$w1"
  local o1 s1; o1="$(bash "$vf" "$w1" "$DUMMY" 2>&1)"; s1="$(status_of "$o1")"; [ -z "$s1" ] && s1=ERROR

  # (2) fixture + reference solution overlaid must PASS
  local w2="$BAT/work-gate/$id-ref"; rm -rf "$w2"; cp -r "$fx" "$w2"; cp -rf "$rs/." "$w2/"
  local o2 s2; o2="$(bash "$vf" "$w2" "$DUMMY" 2>&1)"; s2="$(status_of "$o2")"; [ -z "$s2" ] && s2=ERROR

  if [ "$s1" = "FAIL" ] && [ "$s2" = "PASS" ]; then
    echo "[$id] GATE OK   (fixture=FAIL, refsol=PASS)"
    rm -rf "$w1" "$w2"; return 0
  fi
  echo "[$id] GATE FAILED  fixture=$s1 (want FAIL)  refsol=$s2 (want PASS)"
  [ "$s1" != "FAIL" ] && { echo "  --- fixture verify output ---"; echo "$o1" | sed 's/^/  /'; }
  [ "$s2" != "PASS" ] && { echo "  --- refsol verify output ---"; echo "$o2" | sed 's/^/  /'; }
  return 1
}

ids=("$@")
if [ "${#ids[@]}" -eq 0 ]; then
  mapfile -t ids < <(ls -d "$BAT"/fixtures/Q* 2>/dev/null | xargs -n1 basename | sort)
fi
ok=0; bad=0
for id in "${ids[@]}"; do
  if gate_one "$id"; then ok=$((ok+1)); else bad=$((bad+1)); fi
done
echo "==== GATE SUMMARY: $ok ok, $bad failing ===="
[ "$bad" -eq 0 ]
