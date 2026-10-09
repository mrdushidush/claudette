#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer test: drop full-line comments (indented or not) and blank/whitespace
# lines; keep code lines including ones with a trailing inline '#'.
input='code1
    # indented comment
# comment


code2  # not a stripped comment'
expected='code1
code2  # not a stripped comment'

got=$(printf '%s' "$input" | bash solution.sh 2>/dev/null)
[ "$got" = "$expected" ] || fail "expected [$expected] got [$got]"
pass "comments and blank lines stripped, inline # kept"
