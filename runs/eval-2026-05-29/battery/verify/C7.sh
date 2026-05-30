#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# C7 — debug-from-error: report.js must run clean and print the member names.
out="$(cd "$WORKDIR" && node report.js 2>&1)"; rc=$?
[ "$rc" -eq 0 ] || fail "node report.js exited $rc: $(printf '%s' "$out" | grep -iE 'error' | head -1)"
[ -n "$(printf '%s' "$out" | tr -d '[:space:]')" ] || fail "report.js produced no output"
printf '%s' "$out" | grep -qF -- "Ada" || fail "output did not include planted name 'Ada'"
pass "report.js runs and prints member names (incl. Ada)"
