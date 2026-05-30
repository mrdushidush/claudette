#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
b=$(git -C "$WORKDIR" branch --list "feature/login" 2>/dev/null)
[ -n "$b" ] && pass "branch feature/login exists" || fail "feature/login branch was not created"
