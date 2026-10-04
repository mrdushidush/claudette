#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer test: leading AND trailing whitespace stripped (incl. tabs),
# internal spaces preserved, blank line stays blank.
input=$(printf '  hello  \n\tworld\t\nclean\n\n  a  b  ')
expected=$(printf 'hello\nworld\nclean\n\na  b')

got=$(printf '%s\n' "$input" | bash solution.sh 2>/dev/null)
[ "$got" = "$expected" ] || fail "expected [$expected] got [$got]"
pass "normalize strips both ends, keeps internal spacing"
