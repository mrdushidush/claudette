#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: count-desc ordering, alphabetical tie-break, case folding,
# and mixed whitespace (tabs/newlines) as separators.
chk() { # <expected-multiline> <input>
  local exp got
  exp=$(printf '%b' "$1")
  got=$(printf '%b' "$2" | bash solution.sh 2>/dev/null)
  [ "$got" = "$exp" ] || fail "input $(printf '%q' "$2"): expected [$exp] got [$got]"
}
chk '3 the\n2 cat\n1 dog' 'the cat the DOG the cat'
chk '1 apple\n1 banana\n1 cherry' 'banana apple cherry'
chk '3 hi' 'Hi hi HI'
chk '3 a\n1 b' 'a\tb  a\na'
pass "frequency tally sorted, case-folded, whitespace-split"
