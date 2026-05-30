#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# Run-tests: ground truth is 3 passing tests; agent must report that count.
out=$(cd "$WORKDIR" && python -m unittest -q 2>&1)
rc=$?
ran=$(echo "$out" | grep -oE 'Ran [0-9]+ test' | grep -oE '[0-9]+')
if [ "$rc" -ne 0 ] || [ "$ran" != "3" ]; then
  fail "fixture not green-with-3 (rc=$rc ran=$ran): $(echo "$out" | tail -3 | tr '\n' ' ')"
fi
# Transcript must report the count 3 and indicate success.
if tc 3 && { tc passed || tc ok; }; then
  pass "transcript reports 3 tests passing"
fi
fail "transcript missing '3' and/or passed/ok"
