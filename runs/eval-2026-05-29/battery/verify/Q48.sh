#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: spaces inside an argument must be preserved (not split).
got=$(bash solution.sh a b c 2>/dev/null);        [ "$got" = "a,b,c" ]   || fail "a b c -> '$got'"
got=$(bash solution.sh "a b" c 2>/dev/null);      [ "$got" = "a b,c" ]   || fail "'a b' c -> '$got'"
got=$(bash solution.sh hello 2>/dev/null);        [ "$got" = "hello" ]   || fail "single -> '$got'"
got=$(bash solution.sh "x  y" z 2>/dev/null);     [ "$got" = "x  y,z" ]  || fail "'x  y' z -> '$got'"
got=$(bash solution.sh a "" c 2>/dev/null);       [ "$got" = "a,,c" ]    || fail "empty arg -> '$got'"
pass "arguments joined with commas, spaces preserved"
