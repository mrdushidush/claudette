#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# D4 — multi-file edit: add role to User interface and update label().
cd "$WORKDIR" || fail "cannot cd into workdir"
[ -f src/types.ts ] || fail "src/types.ts missing"
[ -f src/format.ts ] || fail "src/format.ts missing"
[ -f test.ts ] || fail "test.ts missing"

# types.ts must declare the new role field.
grep -qE 'role[[:space:]]*:' src/types.ts || fail "src/types.ts does not declare a 'role' field on User"

out="$(node test.ts 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node test.ts exited $rc: $(printf '%s' "$out" | grep -iE 'fail|error' | head -1)"
pass "User.role added and label() returns 'name (role)'; node test.ts passed"
