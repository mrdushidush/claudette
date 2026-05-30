#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# Multi-file edit: PI made precise in circle.py and whole package stays consistent.
out=$(cd "$WORKDIR" && python -m unittest -q 2>&1)
rc=$?
if [ "$rc" -ne 0 ] || ! echo "$out" | grep -qE '\bOK\b'; then
  fail "geometry tests failing (rc=$rc): $(echo "$out" | tail -3 | tr '\n' ' ')"
fi
if ! grep -qF '3.14159' "$WORKDIR/geometry/circle.py"; then
  fail "circle.py does not contain 3.14159 (PI not updated at source)"
fi
pass "tests pass and circle.py PI updated to 3.14159"
