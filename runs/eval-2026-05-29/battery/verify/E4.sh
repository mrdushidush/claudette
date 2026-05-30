#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# E4 — explain code (TRANSCRIPT check). dedupeAndSort removes duplicates via a map
# set, then sorts ascending via sort.Ints, returning the deduped+sorted slice.
# A correct explanation must mention BOTH behaviours.

# 1) Removing duplicates.
tcre 'duplicat|dedup|unique' || fail "explanation does not mention removing duplicates"

# 2) Sorting / ascending order.
tcre 'sort|ascend|order' || fail "explanation does not mention sorting / ascending order"

pass "explanation covers both deduplication and sorting"
