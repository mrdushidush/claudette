#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# Create-file: utils.py must exist and the clamp test must pass.
if [ ! -f "$WORKDIR/utils.py" ]; then
  fail "utils.py was not created"
fi
out=$(cd "$WORKDIR" && python -m unittest -q 2>&1)
rc=$?
if [ "$rc" -eq 0 ] && echo "$out" | grep -qE '\bOK\b'; then
  pass "utils.py created and clamp test passes: $(echo "$out" | tail -1)"
fi
fail "clamp test failing (rc=$rc): $(echo "$out" | tail -3 | tr '\n' ' ')"
