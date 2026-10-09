#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests (trap-dense): valid order, deterministic alphabetical
# tie-break, cycle -> None, self-loop -> None, disconnected nodes included,
# empty graph -> [], and a node that only appears as a dependency is included.
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import topological_sort

def test_linear():
    assert topological_sort({"b": ["a"], "c": ["b"]}) == ["a", "b", "c"]

def test_diamond_tie_break():
    assert topological_sort({"b": ["a"], "c": ["a"], "d": ["b", "c"]}) == ["a", "b", "c", "d"]

def test_cycle_returns_none():
    assert topological_sort({"a": ["b"], "b": ["a"]}) is None

def test_self_loop_is_cycle():
    assert topological_sort({"a": ["a"]}) is None

def test_disconnected_nodes_included_sorted():
    assert topological_sort({"a": [], "c": [], "b": []}) == ["a", "b", "c"]

def test_empty_graph():
    assert topological_sort({}) == []

def test_dependency_only_node_included():
    assert topological_sort({"y": ["x"]}) == ["x", "y"]
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed|None' | head -4 | tr '\n' ' ')"
fi
pass "all hidden topological_sort tests passed"
