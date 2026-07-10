#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# K7 (edit_file replace_all, refactor) — rename every `trackEvent` -> `recordEvent`
# in analytics.js. Outcome measure (rewards replace_all; a thorough hand-edit that
# finishes in time also passes): ZERO remaining "trackEvent" AND exactly as many
# "recordEvent" as the pristine fixture had "trackEvent". The expected count is
# read from the pristine fixture so it self-adjusts if the fixture changes.
FIX="$(dirname "$0")/../fixtures/K7/analytics.js"
orig=$(grep -oF 'trackEvent' "$FIX" 2>/dev/null | wc -l | tr -d ' ')
f="$WORKDIR/analytics.js"
[ -f "$f" ] || fail "analytics.js missing from workdir"
old=$(grep -oF 'trackEvent' "$f" | wc -l | tr -d ' ')
new=$(grep -oF 'recordEvent' "$f" | wc -l | tr -d ' ')
echo "RECALL: old=$old new=$new/$orig"
if [ "$old" -eq 0 ] && [ "$new" -eq "$orig" ]; then
  pass "renamed all $orig occurrences trackEvent->recordEvent (0 left)"
elif [ "$old" -ne 0 ]; then
  fail "incomplete rename: $old trackEvent still present ($new/$orig recordEvent)"
else
  fail "recordEvent count $new != expected $orig (occurrences added/dropped)"
fi
