#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Normal division to stdout, integer truncation, negatives.
got=$(bash solution.sh 10 2 2>/dev/null); [ "$got" = "5" ] || fail "10/2 expected 5 got '$got'"
got=$(bash solution.sh 7 2 2>/dev/null);  [ "$got" = "3" ] || fail "7/2 expected 3 got '$got'"
got=$(bash solution.sh -6 3 2>/dev/null); [ "$got" = "-2" ] || fail "-6/3 expected -2 got '$got'"

# Division by zero: non-zero exit, and NOTHING on stdout.
out=$(bash solution.sh 5 0 2>/dev/null); rc=$?
[ "$rc" -ne 0 ] || fail "divide-by-zero should exit non-zero (got rc=0)"
[ -z "$out" ] || fail "divide-by-zero should print nothing to stdout (got '$out')"

pass "divides correctly and handles zero divisor cleanly"
