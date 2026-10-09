#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: recursive nested merge, scalar override, keys unique to
# each side preserved, and crucially neither input dict is mutated.
cat > hidden_gate_test.py <<'PY'
import os, sys, copy
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import deep_merge

def test_scalar_override():
    assert deep_merge({"k": 1}, {"k": 2}) == {"k": 2}

def test_disjoint_keys_preserved():
    assert deep_merge({"a": 1}, {"b": 2}) == {"a": 1, "b": 2}

def test_nested_dicts_merge_recursively():
    a = {"cfg": {"x": 1, "y": 2}}
    b = {"cfg": {"y": 3, "z": 4}}
    assert deep_merge(a, b) == {"cfg": {"x": 1, "y": 3, "z": 4}}

def test_inputs_not_mutated():
    a = {"cfg": {"x": 1}}
    b = {"cfg": {"y": 2}}
    a0, b0 = copy.deepcopy(a), copy.deepcopy(b)
    deep_merge(a, b)
    assert a == a0, "first argument was mutated"
    assert b == b0, "second argument was mutated"

def test_dict_replaces_scalar():
    # b's value is a dict where a's is a scalar: b wins wholesale.
    assert deep_merge({"x": 1}, {"x": {"y": 2}}) == {"x": {"y": 2}}

def test_scalar_replaces_dict():
    # b's value is a scalar where a's is a dict: b wins wholesale.
    assert deep_merge({"x": {"y": 2}}, {"x": 5}) == {"x": 5}
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed|mutated' | head -4 | tr '\n' ' ')"
fi
pass "all hidden deep_merge tests passed"
