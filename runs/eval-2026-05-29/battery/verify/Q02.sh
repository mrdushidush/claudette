#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: empty slice, out-of-range on both ends, duplicates
# (must return the LEFTMOST equal position), and a present single value.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q02::lower_bound;
#[test] fn example() { assert_eq!(lower_bound(&[1,2,4,5], 3), 2); }
#[test] fn empty_slice() { assert_eq!(lower_bound(&[], 5), 0); }
#[test] fn below_all() { assert_eq!(lower_bound(&[5,6,7], 1), 0); }
#[test] fn above_all() { assert_eq!(lower_bound(&[1,2,3], 10), 3); }
#[test] fn leftmost_of_duplicates() { assert_eq!(lower_bound(&[1,2,2,2,3], 2), 1); }
#[test] fn present_value() { assert_eq!(lower_bound(&[1,3,5], 3), 1); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all 6 hidden lower_bound tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
