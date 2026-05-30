#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
tc "null pointer" && pass "reported the latest commit subject (null pointer in handler)" || fail "did not report the most-recent commit message"
