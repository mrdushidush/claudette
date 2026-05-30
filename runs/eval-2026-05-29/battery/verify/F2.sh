#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# F2 — create-file: count.sh must print the line count of its first arg.
[ -f "$WORKDIR/count.sh" ] || fail "count.sh was not created in workdir"
[ -f "$WORKDIR/data.txt" ] || fail "data.txt missing from workdir"

out="$(cd "$WORKDIR" && bash count.sh data.txt 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "count.sh exited $rc: $out"

trimmed="$(printf '%s' "$out" | tr -d '[:space:]')"
[ "$trimmed" = "5" ] || fail "expected '5', got: '$out'"

pass "count.sh prints 5 for the 5-line data.txt"
