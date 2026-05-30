#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
tc "notes.txt" && pass "named the modified file notes.txt" || fail "did not identify notes.txt as the uncommitted change"
