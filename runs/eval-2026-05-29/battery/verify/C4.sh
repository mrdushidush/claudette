#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# C4 — create-file: slug.js must exist and make test.js pass.
[ -f "$WORKDIR/slug.js" ] || fail "slug.js was not created"
out="$(cd "$WORKDIR" && node test.js 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node test.js exited $rc: $(printf '%s' "$out" | grep -iE 'error|expected' | head -1)"
pass "slug.js created and test.js passes"
