#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# C3 — run-tests: tests stay green AND agent reports the count.
out="$(cd "$WORKDIR" && node test.js 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node test.js exited $rc: $(printf '%s' "$out" | grep -iE 'error|expected' | head -1)"
tc "5" || fail "transcript does not mention 5 (the number that passed)"
{ tc "passed" || tc "pass"; } || fail "transcript does not say pass/passed"
pass "tests green and agent reported 5 passed"
