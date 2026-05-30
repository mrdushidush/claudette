#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/tcre/tcount

# Answer-from-codebase: code uses MAX_ATTEMPTS=2; docstring lies ("up to 5 times").
# Agent must trust the SOURCE and answer 2, not 5.

# Must state the real value 2.
if ! tc 2; then
  fail "transcript never mentions the real value 2"
fi
# Must anchor 2 as the ACTUAL count, not merely quote the docstring's "5 times".
# Anchoring proves the agent trusted the SOURCE (MAX_ATTEMPTS=2 / range loop).
# We require an anchor rather than a broad anti-5 heuristic, because a correct
# answer often legitimately quotes "up to 5 times" from the docstring while
# concluding 2 -- so blanket-rejecting "5" produces false negatives.
if ! { tc MAX_ATTEMPTS || tc twice || tcre '2 times' || tcre 'actually .{0,30}\b2\b' || tcre 'really .{0,30}\b2\b' || tcre 'retr(y|ies)?.{0,20}\b2\b' || tcre '\b2\b.{0,20}(time|attempt|retr)'; }; then
  fail "mentions 2 but does not anchor it as the actual retry count"
fi
pass "transcript identifies the real retry count as 2 (MAX_ATTEMPTS)"
