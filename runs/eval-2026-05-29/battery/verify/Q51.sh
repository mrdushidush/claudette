#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests (trap-dense): combined units, all three units, whitespace
# tolerance, order-independence, large values, and STRICT validation — an invalid
# or empty string, or one with stray characters, must raise ValueError (not guess).
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import pytest
from solution import parse_duration

def test_combined_all_units():
    assert parse_duration("1h30m15s") == 5415

def test_single_units():
    assert parse_duration("2h") == 7200
    assert parse_duration("30m") == 1800
    assert parse_duration("45s") == 45

def test_whitespace_tolerated():
    assert parse_duration("1h 30m") == 5400

def test_order_independent():
    assert parse_duration("30m1h") == 5400

def test_large_value():
    assert parse_duration("100h") == 360000

def test_invalid_raises():
    with pytest.raises(ValueError):
        parse_duration("abc")

def test_empty_raises():
    with pytest.raises(ValueError):
        parse_duration("")

def test_stray_characters_raise():
    with pytest.raises(ValueError):
        parse_duration("1h2x")

def test_bare_number_raises():
    # no unit suffix -> not a valid duration
    with pytest.raises(ValueError):
        parse_duration("45")
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed|raises' | head -4 | tr '\n' ' ')"
fi
pass "all hidden parse_duration tests passed"
