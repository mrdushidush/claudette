#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: correctness edges (needs two DISTINCT positions, so a
# single element equal to half the target is not a pair; equal values at two
# positions are), plus a large all-distinct input that only a ~linear solution
# finishes quickly.
cat > hidden_gate_test.py <<'PY'
import os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import has_pair_with_sum

def test_basic_pair():
    assert has_pair_with_sum([1, 2, 3, 9], 12) is True

def test_no_pair():
    assert has_pair_with_sum([1, 2, 3], 100) is False

def test_needs_two_distinct_positions():
    assert has_pair_with_sum([3], 6) is False

def test_equal_values_two_positions():
    assert has_pair_with_sum([3, 3], 6) is True

def test_negatives():
    assert has_pair_with_sum([-5, 1, 7], 2) is True

def test_empty():
    assert has_pair_with_sum([], 0) is False

def test_large_input_is_fast():
    nums = list(range(0, 32000, 2))  # 16000 distinct evens
    start = time.perf_counter()
    got = has_pair_with_sum(nums, 9999999)  # odd target -> impossible
    elapsed = time.perf_counter() - start
    assert got is False
    assert elapsed < 2.0, f"too slow: {elapsed:.2f}s (needs a linear approach)"
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed|slow' | head -4 | tr '\n' ' ')"
fi
pass "correct and fast on large input"
