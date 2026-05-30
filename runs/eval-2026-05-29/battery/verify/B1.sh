#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# Bugfix: mean must be correct so the shipped test passes.
out=$(cd "$WORKDIR" && python -m unittest -q 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && echo "$out" | grep -qE '\bOK\b'; then
  pass "test_stats passes (mean fixed): $(echo "$out" | tail -1)"
fi
fail "test_stats still failing (rc=$rc): $(echo "$out" | tail -3 | tr '\n' ' ')"
