#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: tax must be applied to the subtotal (rounded once), so
# per-item rounding drift disappears; no-drift/single/zero-rate cases stay stable.
cat > hidden_gate_test.py <<'PY'
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from invoice import invoice_total

def test_tax_on_subtotal_not_per_item():
    assert invoice_total([33, 33, 33], 10) == 109

def test_another_drift_case():
    assert invoice_total([17, 17, 17, 17], 10) == 75

def test_no_drift_case():
    assert invoice_total([100, 100], 10) == 220

def test_single_item():
    assert invoice_total([50], 10) == 55

def test_zero_rate():
    assert invoice_total([10, 20, 30], 0) == 60
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'assert|error|failed' | head -4 | tr '\n' ' ')"
fi
pass "tax applied on subtotal, no per-item drift"
