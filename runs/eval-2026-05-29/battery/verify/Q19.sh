#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: the 1024 boundary, one-decimal rounding, and scaling all
# the way up through MB/GB/TB (not just KB).
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from solution import human_readable_size

def test_below_kb():
    assert human_readable_size(0) == "0 B"
    assert human_readable_size(1023) == "1023 B"

def test_kb_boundary():
    assert human_readable_size(1024) == "1.0 KB"
    assert human_readable_size(1536) == "1.5 KB"

def test_rounds_to_one_decimal():
    assert human_readable_size(1500) == "1.5 KB"

def test_scales_up():
    assert human_readable_size(1048576) == "1.0 MB"
    assert human_readable_size(1073741824) == "1.0 GB"
    assert human_readable_size(1099511627776) == "1.0 TB"
    assert human_readable_size(1125899906842624) == "1.0 PB"
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "all hidden size-format tests passed"
