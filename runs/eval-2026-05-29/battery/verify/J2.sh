#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
subj=$(git -C "$WORKDIR" log -1 --pretty=%s 2>/dev/null)
tracked=$(git -C "$WORKDIR" ls-files greet.py 2>/dev/null)
if echo "$subj" | grep -iqF "Add greeting" && [ -n "$tracked" ]; then
  pass "committed greet.py with subject: $subj"
else
  fail "HEAD subject='$subj' tracked='$tracked' (expected commit 'Add greeting...' including greet.py)"
fi
