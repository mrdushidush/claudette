#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # gives: WORKDIR=$1  TRANSCRIPT=$2 ; and fns pass/fail/tc/tcre/tcount

cd "$WORKDIR" || fail "cannot cd into workdir"

out=$(cargo build --quiet 2>&1)
if [ $? -eq 0 ]; then
  pass "cargo build succeeded (type error fixed)"
else
  fail "cargo build still failing:\n$out"
fi
