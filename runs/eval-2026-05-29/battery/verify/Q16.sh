#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: exact-multiple must NOT yield a trailing empty page,
# empty input yields [], plus partial-last, page-bigger-than-list, one-per-page.
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import paginate

def test_exact_multiple_no_trailing_empty():
    assert paginate([1, 2, 3, 4], 2) == [[1, 2], [3, 4]]

def test_partial_last_page():
    assert paginate([1, 2, 3, 4, 5], 2) == [[1, 2], [3, 4], [5]]

def test_empty_input():
    assert paginate([], 3) == []

def test_page_larger_than_list():
    assert paginate([1, 2], 5) == [[1, 2]]

def test_one_per_page():
    assert paginate([1, 2, 3], 1) == [[1], [2], [3]]
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "all hidden paginate tests passed"
