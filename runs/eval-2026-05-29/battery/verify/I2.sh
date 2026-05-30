#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
gt=(Assistant Planner Router Coder TestCoder Verifier SurgicalCoder Cto)
n=0; for t in "${gt[@]}"; do grep -iwqF -- "$t" "$TRANSCRIPT" && n=$((n+1)); done
echo "RECALL: $n/8"
[ "$n" -ge 7 ] && pass "enumerated $n/8 forge roles" || fail "only $n/8 forge roles (need >=7; enumeration weak spot)"
