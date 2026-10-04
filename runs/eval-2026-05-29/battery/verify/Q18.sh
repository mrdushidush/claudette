#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: calls without a bucket must not share state; an explicit
# bucket is still appended to.
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import add_item

def test_fresh_bucket_each_call():
    assert add_item(1) == [1]
    assert add_item(2) == [2]

def test_explicit_bucket_used():
    assert add_item(3, [10]) == [10, 3]

def test_independent_lists():
    a = add_item("a")
    b = add_item("b")
    assert a == ["a"]
    assert b == ["b"]
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "buckets are independent per call"
