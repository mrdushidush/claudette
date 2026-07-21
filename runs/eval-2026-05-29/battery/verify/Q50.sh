#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: comma-split field extraction, first field, missing field
# (fewer columns) -> empty, empty field -> empty, and multi-line.
chk() { # <expected-multiline> <col> <input>
  local exp got
  exp=$(printf '%b' "$1")
  got=$(printf '%b' "$3" | bash solution.sh "$2" 2>/dev/null)
  [ "$got" = "$exp" ] || fail "col $2 on $(printf '%q' "$3"): expected [$exp] got [$got]"
}
chk 'b' 2 'a,b,c\n'
chk 'a' 1 'a,b,c\n'
chk '' 5 'a,b\n'
chk '' 2 'a,,c\n'
chk 'z\n3' 3 'x,y,z\n1,2,3\n'
pass "extracts Nth comma field, handles missing/empty fields"
