#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: trailing newline, NO trailing newline, single unterminated
# line, and empty input.
chk() { # <expected> <printf-arg>
  local exp="$1" in="$2" got
  got=$(printf '%b' "$in" | bash solution.sh 2>/dev/null)
  [ "$got" = "$exp" ] || fail "input $(printf '%q' "$in"): expected $exp, got '$got'"
}
chk 3 'a\nb\nc\n'
chk 3 'a\nb\nc'
chk 1 'x'
chk 0 ''
pass "line counts correct with and without trailing newline"
