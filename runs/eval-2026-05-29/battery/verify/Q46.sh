#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Ten numbered lines "1".."10".
INPUT=$(printf '%s\n' 1 2 3 4 5 6 7 8 9 10)

chk() { # <expected-multiline> <start> <end>
  local exp got
  exp=$(printf '%b' "$1")
  got=$(printf '%s\n' "$INPUT" | bash solution.sh "$2" "$3" 2>/dev/null)
  [ "$got" = "$exp" ] || fail "range $2..$3: expected [$exp] got [$got]"
}
chk '3\n4\n5' 3 5      # mid range
chk '4' 4 4            # single line
chk '1\n2' 1 2         # from start
chk '8\n9\n10' 8 100   # end beyond EOF
chk '' 20 30           # start beyond EOF
pass "inclusive line range correct including edges"
