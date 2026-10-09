#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: inclusive ranges, sorted+deduped output across overlapping
# ranges, unordered input, and tolerance of surrounding whitespace.
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import parse_ranges

def test_inclusive_range():
    assert parse_ranges("1-3") == [1, 2, 3]

def test_mixed():
    assert parse_ranges("1-3,5,8-10") == [1, 2, 3, 5, 8, 9, 10]

def test_dedupe_and_sort_overlaps():
    assert parse_ranges("5,1-3,2-4") == [1, 2, 3, 4, 5]

def test_whitespace_tolerated():
    assert parse_ranges(" 1 - 3 , 5 ") == [1, 2, 3, 5]

def test_unordered_singletons():
    assert parse_ranges("3,1,2") == [1, 2, 3]

def test_single_element_range():
    assert parse_ranges("4-4") == [4]
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "all hidden parse_ranges tests passed"
