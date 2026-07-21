#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: quoted commas, escaped ("") quotes, empty and quoted-empty
# fields, and a trailing empty field.
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import parse_csv_line

def test_plain():
    assert parse_csv_line("x,y,z") == ["x", "y", "z"]

def test_quoted_comma():
    assert parse_csv_line('a,"b,c",d') == ["a", "b,c", "d"]

def test_escaped_quote():
    assert parse_csv_line('"she said ""hi"""') == ['she said "hi"']

def test_trailing_empty_field():
    assert parse_csv_line("a,b,") == ["a", "b", ""]

def test_quoted_empty_field():
    assert parse_csv_line('a,"",c') == ["a", "", "c"]

def test_unquoted_spaces_preserved():
    # RFC-4180: spaces are part of an unquoted field; don't trim them.
    assert parse_csv_line("a, b ,c") == ["a", " b ", "c"]
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "all hidden CSV tests passed"
