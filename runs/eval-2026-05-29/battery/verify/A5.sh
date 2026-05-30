#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # gives: WORKDIR=$1  TRANSCRIPT=$2 ; and fns pass/fail/tc/tcre/tcount

cd "$WORKDIR" || fail "cannot cd into workdir"

cfg="src/config.rs"
[ -f "$cfg" ] || fail "src/config.rs missing"

# The constant must have been raised to 5 in config.rs.
if ! grep -qE 'MAX_RETRIES[[:space:]]*:[[:space:]]*u32[[:space:]]*=[[:space:]]*5' "$cfg"; then
  fail "MAX_RETRIES: u32 = 5 not found in src/config.rs"
fi

# Everything still lines up: the retry test now passes.
out=$(cargo test --quiet 2>&1)
if [ $? -eq 0 ] && echo "$out" | grep -qE 'test result: ok'; then
  pass "MAX_RETRIES raised to 5; retry test passes"
else
  fail "cargo test failed:\n$out"
fi
