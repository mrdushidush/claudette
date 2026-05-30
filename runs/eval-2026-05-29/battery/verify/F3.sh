#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# F3 — explain code: the transcript must convey what the pipeline DOES.
# Match on EXPLANATORY language only, never on bare command tokens
# (uniq/head/sort/$1/5) that would leak verbatim from the file contents —
# otherwise a transcript that merely echoes pipeline.sh would falsely pass.
# Three concept groups; require >=2 of 3, with the "count" group mandatory.
g_field='first (column|field)|\bIP\b|ip address|address|client'
g_count='count|frequenc|how many|occurr|number of times|tally|how often|times each'
g_top='top|most (frequent|common)|five most|5 most|highest|largest count|descending'

n=0
tcre "$g_field" && n=$((n+1))
tcre "$g_count" && n=$((n+1))
tcre "$g_top"   && n=$((n+1))

# Count notion is the essential idea; demand it explicitly.
tcre "$g_count" || fail "explanation does not convey counting unique occurrences"
[ "$n" -ge 2 ] || fail "explanation covers only $n/3 concepts (need >=2: field, count, top-N)"

pass "explanation conveys $n/3 concepts incl. counting unique occurrences"
