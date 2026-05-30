#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # gives: WORKDIR=$1  TRANSCRIPT=$2 ; and fns pass/fail/tc/tcre/tcount

# Transcript-based: explanation must capture both the dedup concept
# AND the key nuance that only CONSECUTIVE/adjacent duplicates are removed.

if ! tcre 'dedup|de-dup|duplicat|repeat'; then
  fail "explanation does not mention the duplicate/dedup concept"
fi

if tcre 'consecutive|adjacent|in a row|neighbo|back[- ]?to[- ]?back|run of'; then
  pass "explanation covers dedup of consecutive/adjacent duplicates"
else
  fail "explanation omits that only consecutive/adjacent duplicates are removed"
fi
