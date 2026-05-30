#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1 TRANSCRIPT=$2 ; pass/fail/tc/tcre/tcount

# E2 — refactor/rename (SOURCE-LEVEL, no compile). GetUserName must be renamed to
# DisplayName everywhere: the definition in user.go and BOTH call sites in main.go.
# We grep the resulting source across the whole workdir.
#
# rg-or-grep shim (see E1): prefer rg, else GNU grep -rn.
[ -f "$WORKDIR/user.go" ] || fail "user.go not found in workdir"
[ -f "$WORKDIR/main.go" ] || fail "main.go not found in workdir"

search() { # search <pattern> <file-or-dir> : prints matching lines
  if command -v rg >/dev/null 2>&1; then
    rg -n --no-heading -- "$1" "$2" 2>/dev/null
  else
    grep -rn -- "$1" "$2" 2>/dev/null
  fi
}

# 1) Old name must be COMPLETELY gone across the workdir.
if [ -n "$(search 'GetUserName' "$WORKDIR")" ]; then
  fail "old name 'GetUserName' still present:\n$(search 'GetUserName' "$WORKDIR")"
fi

# 2) New definition must exist in user.go.
if [ -z "$(search 'func DisplayName' "$WORKDIR/user.go")" ]; then
  fail "renamed definition 'func DisplayName' not found in user.go"
fi

# 3) DisplayName( must appear at least 3 times total (1 defn + 2 call sites).
n=$(search 'DisplayName(' "$WORKDIR" | wc -l)
if [ "$n" -lt 3 ]; then
  fail "expected >=3 occurrences of 'DisplayName(' (defn + 2 calls); found $n"
fi

pass "GetUserName renamed to DisplayName everywhere (defn in user.go + 2 call sites; $n total)"
