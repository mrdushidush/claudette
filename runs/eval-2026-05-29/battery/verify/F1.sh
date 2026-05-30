#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# F1 — bugfix: greet.sh must print "Hello, NAME!" (with the "!") for each arg.
[ -f "$WORKDIR/greet.sh" ] || fail "greet.sh missing in workdir"

out="$(cd "$WORKDIR" && bash greet.sh Ada Grace 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "greet.sh exited $rc: $out"

printf '%s\n' "$out" | grep -qx 'Hello, Ada!'   || fail "missing line 'Hello, Ada!' (got: $out)"
printf '%s\n' "$out" | grep -qx 'Hello, Grace!' || fail "missing line 'Hello, Grace!' (got: $out)"

pass "greet.sh prints 'Hello, Ada!' and 'Hello, Grace!'"
