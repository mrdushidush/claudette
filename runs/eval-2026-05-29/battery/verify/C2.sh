#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# C2 — add-feature: applyDiscount added+exported so test.js passes.
out="$(cd "$WORKDIR" && node test.js 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node test.js exited $rc: $(printf '%s' "$out" | grep -iE 'error|expected' | head -1)"
pass "test.js passes (applyDiscount added)"
