#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # gives: WORKDIR=$1  TRANSCRIPT=$2 ; and fns pass/fail/tc/tcre/tcount

cd "$WORKDIR" || fail "cannot cd into workdir"

out=$(cargo test --quiet 2>&1)
if [ $? -eq 0 ] && echo "$out" | grep -qE 'test result: ok'; then
  pass "cargo test succeeded (median added)"
else
  fail "cargo test failed:\n$out"
fi
