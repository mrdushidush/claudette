#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
gt=(CLAUDETTE_FORGE_ABORT_WINDOW_SECS CLAUDETTE_FORGE_ALLOW_DIRTY CLAUDETTE_FORGE_AUTO_APPROVE CLAUDETTE_FORGE_SECURITY_OVERRIDE CLAUDETTE_FORGE_SECURITY_REVIEW CLAUDETTE_FORGE_SUBMIT_ON_FAIL)
n=0; for t in "${gt[@]}"; do grep -iqF -- "$t" "$TRANSCRIPT" && n=$((n+1)); done
echo "RECALL: $n/6"
[ "$n" -ge 5 ] && pass "enumerated $n/6 CLAUDETTE_FORGE_ vars" || fail "only $n/6 CLAUDETTE_FORGE_ vars (need >=5; enumeration weak spot)"
