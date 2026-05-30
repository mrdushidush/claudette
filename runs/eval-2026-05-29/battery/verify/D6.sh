#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# D6 — run-tests: test stays green AND the agent reports the count (6 passed).
out="$(cd "$WORKDIR" && node test.ts 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node test.ts exited $rc: $(printf '%s' "$out" | grep -iE 'fail|error' | head -1)"
tc "6" || fail "transcript does not mention 6 (the number of tests that passed)"
{ tc "passed" || tc "pass"; } || fail "transcript does not say pass/passed"
pass "tests green and agent reported 6 passed"
