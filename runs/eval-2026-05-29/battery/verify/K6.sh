#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# K6 (repo_map Ruby, bugfix) — passing?(score) must treat exactly 60 as passing:
# `score >= 60` (or `60 <= score`). The bug is `score > 60`. No Ruby run for
# correctness — verify by FILE STATE, scoped to the passing? method body (the
# letter() method legitimately contains ">= 60" already, so it must be excluded).
cd "$WORKDIR" || fail "cannot cd into workdir"
[ -f grader.rb ] || fail "grader.rb missing from workdir"
body=$(awk '/def self\.passing\?/,/end/' grader.rb)
if printf '%s' "$body" | grep -Eq -e '>= ?60' -e '60 ?<='; then
  pass "passing? now includes exactly 60 (inclusive >= 60)"
elif printf '%s' "$body" | grep -Eq -e '> ?60'; then
  fail "passing? still uses > 60 — a score of 60 is wrongly excluded"
else
  fail "passing? not fixed to an inclusive >= 60 test"
fi
