#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # gives: WORKDIR=$1  TRANSCRIPT=$2 ; and fns pass/fail/tc/tcre/tcount

cd "$WORKDIR" || fail "cannot cd into workdir"

# Confirm the suite really is green with exactly 4 passing tests.
out=$(cargo test --quiet 2>&1)
if [ $? -ne 0 ] || ! echo "$out" | grep -qE 'test result: ok'; then
  fail "cargo test did not pass in workdir:\n$out"
fi
if ! echo "$out" | grep -qE 'test result: ok\. 4 passed'; then
  fail "expected exactly 4 passing tests; got:\n$(echo "$out" | grep -E 'test result')"
fi

# Transcript must report the count (4) and a pass/ok word.
if tc 4 && { tc passed || tc ok; }; then
  pass "transcript reports 4 tests passed"
else
  fail "transcript missing the count 4 and/or a pass/ok word"
fi
