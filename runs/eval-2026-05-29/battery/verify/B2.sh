#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# Add-feature: variance() must exist and the test must pass.
out=$(cd "$WORKDIR" && python -m unittest -q 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && echo "$out" | grep -qE '\bOK\b'; then
  pass "variance added, test passes: $(echo "$out" | tail -1)"
fi
fail "variance test failing (rc=$rc): $(echo "$out" | tail -3 | tr '\n' ' ')"
