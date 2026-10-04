#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: correctness at the window boundaries (k==0, exactly-k,
# just-over-k, adjacent), PLUS a large all-distinct input with a big k that only
# a roughly-linear implementation can finish quickly. The naive O(n*k) fixture
# takes many seconds; an O(n) sliding-window/set approach is well under a second.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q12::has_duplicate_within_k;
#[test] fn k_zero_never_matches() { assert!(!has_duplicate_within_k(&[1, 1], 0)); }
#[test] fn exactly_k_apart() { assert!(has_duplicate_within_k(&[1, 2, 3, 1], 3)); }
#[test] fn just_over_k() { assert!(!has_duplicate_within_k(&[1, 2, 3, 1], 2)); }
#[test] fn adjacent() { assert!(has_duplicate_within_k(&[5, 5], 1)); }
#[test] fn no_duplicates() { assert!(!has_duplicate_within_k(&[1, 2, 3, 4], 10)); }
#[test]
fn large_input_must_be_fast() {
    let n = 200_000usize;
    let data: Vec<i64> = (0..n as i64).collect(); // all distinct -> false, forces a full scan
    let start = std::time::Instant::now();
    let got = has_duplicate_within_k(&data, 100_000);
    let elapsed = start.elapsed();
    assert!(!got);
    assert!(
        elapsed.as_secs_f64() < 2.5,
        "too slow ({elapsed:?}); needs a roughly-linear approach"
    );
}
RS

out=$(cargo test --release --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|too slow|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "correct and fast on large input" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
