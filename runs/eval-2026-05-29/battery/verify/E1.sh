#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# E1 — bugfix (SOURCE-LEVEL, no compile). Planted bug in calc.go: Max returns
# the SMALLER value (`if a < b { return a }; return b`). Correct fix returns the
# larger. We grep the resulting source: the buggy line must be GONE and some
# max-returning form must be present.
#
# rg-or-grep shim: the eval contract names `rg`, but verifiers run in a plain
# bash subprocess where rg may not be on PATH. Fall back to GNU `grep -Pz`
# (multiline Perl regex), which is what rg -U effectively does here.
F="$WORKDIR/calc.go"
[ -f "$F" ] || fail "calc.go not found in workdir"

m() { # m <pcre-multiline-pattern> : true if pattern matches across lines
  if command -v rg >/dev/null 2>&1; then
    rg -Uq "$1" "$F"
  else
    grep -Pzoq "$1" "$F"
  fi
}

# 1) The buggy form must be eliminated: `if a < b {` immediately followed by `return a`.
if m 'if a < b \{\s*return a'; then
  fail "buggy line still present: 'if a < b { return a' (returns the smaller value)"
fi

# 2) Some max-returning form must be present. Accept canonical fixes:
#    - flip condition: if a > b { return a   (then return b)
#    - flip returns:   if a < b { return b   (then return a)
#    - if a >= b ...
#    - a math.Max-style rewrite (math.Max / explicit larger comparison)
if m 'if a > b \{\s*return a' \
   || m 'if a < b \{\s*return b' \
   || m 'if a >= b \{\s*return a' \
   || m 'if b < a \{\s*return a' \
   || m 'if b > a \{\s*return b' \
   || m 'math\.Max'; then
  pass "calc.go Max fixed to return the larger value (buggy line removed)"
fi

fail "no max-returning pattern found in calc.go after removing the buggy line"
