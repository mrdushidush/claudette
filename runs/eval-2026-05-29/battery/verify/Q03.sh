#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: the remainder must be distributed (shares sum to total
# and differ by at most 1), plus the degenerate people==0 case must not panic.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q03::split_bill;
fn sum(v: &[u64]) -> u64 { v.iter().sum() }
#[test] fn even() { assert_eq!(split_bill(100, 4), vec![25,25,25,25]); }
#[test] fn remainder_sums_exact_1() { let v = split_bill(100, 3); assert_eq!(sum(&v), 100); assert_eq!(v.len(), 3); assert!(v.iter().max().unwrap() - v.iter().min().unwrap() <= 1); }
#[test] fn remainder_sums_exact_2() { let v = split_bill(10, 4); assert_eq!(sum(&v), 10); assert_eq!(v.len(), 4); assert!(v.iter().max().unwrap() - v.iter().min().unwrap() <= 1); }
#[test] fn single_person_gets_all() { assert_eq!(split_bill(5, 1), vec![5]); }
#[test] fn zero_total() { assert_eq!(split_bill(0, 3), vec![0,0,0]); }
#[test] fn zero_people_no_panic() { assert_eq!(split_bill(7, 0), Vec::<u64>::new()); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED|overflow' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all 6 hidden split_bill tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
