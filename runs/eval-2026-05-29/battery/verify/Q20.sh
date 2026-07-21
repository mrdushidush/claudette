#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: "present" means "not None" — falsy-but-real values like
# 0, "", and False must be KEPT (a plain `if v:` truthiness check wrongly drops them).
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import keep_present

def test_drops_none_only():
    assert keep_present([1, None, 3, None]) == [1, 3]

def test_keeps_zero():
    assert keep_present([0, 1, 2]) == [0, 1, 2]

def test_keeps_empty_string():
    assert keep_present(["", "a"]) == ["", "a"]

def test_keeps_false():
    assert keep_present([False, True]) == [False, True]

def test_mixed_falsy_and_none():
    assert keep_present([0, None, "", None, 5]) == [0, "", 5]
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "keeps falsy-but-present values, drops only None"
