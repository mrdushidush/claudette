#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# Debug-from-error: app.py must run cleanly and report 'l' counted twice.
out=$(cd "$WORKDIR" && python app.py 2>&1)
rc=$?
if [ "$rc" -ne 0 ]; then
  fail "app.py still crashing (rc=$rc): $(echo "$out" | tail -3 | tr '\n' ' ')"
fi
# Lenient on dict formatting: accept "'l': 2", "'l':2", or any "l ... : 2".
if echo "$out" | grep -qE "'l': ?2|l.*: ?2"; then
  pass "app.py runs and counts 'l' as 2: $out"
fi
fail "app.py ran (rc=0) but output lacks l-count of 2: $out"
