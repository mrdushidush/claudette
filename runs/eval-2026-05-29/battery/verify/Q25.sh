#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: exact cent totals including the classic float traps
# (4.35 -> 435, 19.99 -> 1999), plus sub-dollar and empty cases.
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import total_cents

def test_whole_dollars():
    assert total_cents(["1.00", "2.00"]) == 300

def test_float_trap_435():
    assert total_cents(["4.35"]) == 435

def test_float_trap_1999():
    assert total_cents(["19.99"]) == 1999

def test_sub_dollar():
    assert total_cents(["0.10", "0.20", "0.05"]) == 35

def test_mixed_list():
    assert total_cents(["4.35", "19.99", "0.01"]) == 2435

def test_empty():
    assert total_cents([]) == 0

def test_integer_dollar_string():
    # a price with no decimal point still means whole dollars
    assert total_cents(["5"]) == 500
    assert total_cents(["5", "0.99"]) == 599
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "cent totals are exact"
