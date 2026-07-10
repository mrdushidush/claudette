#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# K2 (repo_map C++, bugfix) — celsiusToFahrenheit must ADD 32 (F = C*9/5 + 32);
# the bug subtracts it. No C++ toolchain on this box, so verify by FILE STATE:
# the fixed source must contain "+ 32" and no longer contain "- 32". The literal
# "32" appears nowhere else in the fixture, so this is unambiguous.
cd "$WORKDIR" || fail "cannot cd into workdir"
if grep -rqE -e '\+ ?32' . && ! grep -rqE -e '- ?32' .; then
  pass "celsiusToFahrenheit now adds 32 (bug fixed)"
elif grep -rqE -e '- ?32' .; then
  fail "conversion still subtracts 32 — bug not fixed"
else
  fail "conversion no longer adds 32 as expected"
fi
