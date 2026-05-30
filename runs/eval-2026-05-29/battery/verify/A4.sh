#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # gives: WORKDIR=$1  TRANSCRIPT=$2 ; and fns pass/fail/tc/tcre/tcount

cd "$WORKDIR" || fail "cannot cd into workdir"

src="src/lib.rs"
[ -f "$src" ] || fail "src/lib.rs missing"

# Old function name must be gone (definition and call sites).
if grep -qE 'fn[[:space:]]+add[[:space:]]*\(' "$src"; then
  fail "old definition 'fn add(' still present in src/lib.rs"
fi
if grep -qE '(^|[^_[:alnum:]])add[[:space:]]*\(' "$src"; then
  fail "an 'add(' call site still remains in src/lib.rs"
fi

# New function name must exist as a definition and be called.
if ! grep -qE 'fn[[:space:]]+sum2[[:space:]]*\(' "$src"; then
  fail "renamed definition 'fn sum2(' not found in src/lib.rs"
fi
if ! grep -qE 'sum2[[:space:]]*\(' "$src"; then
  fail "no 'sum2(' call site found in src/lib.rs"
fi

# Behavior preserved: tests still pass.
out=$(cargo test --quiet 2>&1)
if [ $? -eq 0 ] && echo "$out" | grep -qE 'test result: ok'; then
  pass "add renamed to sum2 throughout; tests pass"
else
  fail "cargo test failed after rename:\n$out"
fi
