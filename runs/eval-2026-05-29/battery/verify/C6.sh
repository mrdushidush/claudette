#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# C6 — explain code: must convey (1) grouping, (2) an object/map/dictionary
# return value, and (3) that the values are collections of items (arrays /
# lists / "buckets"). Checked as three independent signals so a correct
# explanation that says "buckets" instead of "arrays" still passes, while a
# shallow/wrong answer (no object-return, no collection-of-items) still fails.
tcre "group" || fail "no mention of grouping"
tcre 'object|map|dictionary|record|\{\}' \
  || fail "did not describe the object/map/dictionary return value"
tcre 'bucket|array|list|\[\]|collection|push|items? (in|into|under|per|grouped)|each (key|group).{0,30}(item|value)' \
  || fail "did not convey that values are collections (arrays/lists/buckets) of items"
pass "explanation covers grouping, object return, and collection-valued buckets"
