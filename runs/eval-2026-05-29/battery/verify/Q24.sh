#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: greedy fill, an over-long word on its own line, exact-width
# fit, empty input, and whitespace normalization (collapse runs, trim, tabs) — a
# naive split(' ') approach fails the whitespace cases.
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import word_wrap

def test_basic():
    assert word_wrap("the quick brown fox", 9) == "the quick\nbrown fox"

def test_long_word_own_line():
    assert word_wrap("a supercalifragilistic b", 5) == "a\nsupercalifragilistic\nb"

def test_exact_fit():
    assert word_wrap("ab cd", 5) == "ab cd"

def test_empty():
    assert word_wrap("", 10) == ""

def test_collapses_multiple_spaces():
    assert word_wrap("hello   world", 20) == "hello world"

def test_trims_and_handles_tabs():
    assert word_wrap("  a\tb  ", 20) == "a b"
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "all hidden word_wrap tests passed"
