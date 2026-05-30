#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
gt=(resume telegram tui forge doctor auth-google revoke)
n=0; for t in "${gt[@]}"; do grep -iqF -- "$t" "$TRANSCRIPT" && n=$((n+1)); done
echo "RECALL: $n/7"
[ "$n" -ge 6 ] && pass "enumerated $n/7 CLI modes" || fail "only $n/7 CLI modes (need >=6; enumeration weak spot)"
