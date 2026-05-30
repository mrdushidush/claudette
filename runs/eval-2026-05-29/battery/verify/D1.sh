#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# D1 — bugfix: emails without an "@" must be rejected. Verifier runs the test.
cd "$WORKDIR" || fail "cannot cd into workdir"
[ -f validate.ts ] || fail "validate.ts missing"
[ -f test.ts ] || fail "test.ts missing"

# Guard: the test must not have been edited away (prompt said "Don't edit the test").
grep -q "isValidEmail('foo.com')" test.ts || fail "test.ts no longer asserts the foo.com case — test appears edited"

out="$(node test.ts 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node test.ts exited $rc: $(printf '%s' "$out" | grep -iE 'fail|error' | head -1)"
pass "node test.ts passed — emails without @ are now rejected"
