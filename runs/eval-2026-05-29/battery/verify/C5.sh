#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# C5 — multi-file edit: TAX_RATE set to 0.08 in config.js so withTax(100)===108.
out="$(cd "$WORKDIR" && node test.js 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node test.js exited $rc: $(printf '%s' "$out" | grep -iE 'error|expected' | head -1)"
grep -qF -- "0.08" "$WORKDIR/config.js" || fail "config.js does not contain 0.08"
pass "tax rate updated to 0.08 and test.js passes"
