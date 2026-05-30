#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
# GT: loop-breaker / search-budget nudge lives in runtime/conversation.rs
tc "conversation.rs" && pass "named conversation.rs" || fail "did not name conversation.rs"
