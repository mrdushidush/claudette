#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# C3 — run-tests: tests stay green AND agent reports the count.
out="$(cd "$WORKDIR" && node test.js 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node test.js exited $rc: $(printf '%s' "$out" | grep -iE 'error|expected' | head -1)"
# Word-boundary match (issue #176): the bare substring "5" matched the "5" in
# "1,052" from an enclosing project's suite, masking the run_tests
# workspace-escape defect since 2026-05-30. Require a standalone 5.
tcre '\b5\b' || fail "transcript does not report a standalone 5 (the number that passed)"
{ tc "passed" || tc "pass"; } || fail "transcript does not say pass/passed"
pass "tests green and agent reported 5 passed"
