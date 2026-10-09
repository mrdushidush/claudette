#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# TIER 2 — axis: context pressure.
# The defect is a single comparison operator in app/shipping.py, buried in an
# 11-module package. The prompt gives only a symptom (a wrong total), never a
# location, so the task grades whether the model can FIND the relevant code
# before changing it. The fix itself is trivial once located.
#
# The regression assertions matter as much as the boundary one: a shotgun edit
# that lowers the threshold, drops the fee, or moves tax onto shipping also
# produces 53.50 for the sample order, and must still fail here.
cat > hidden_gate_test.py <<'PY'
import subprocess
import sys

import pytest

from app.models import Customer, Item, Order
from app.orders import compute_totals
from app.receipt import format_money
from app.shipping import qualifies_for_free_shipping
from app.validation import validate_order


def order_worth(amount, tier="standard"):
    customer = Customer("C-X", "Buyer", tier=tier)
    return Order("ORD-X", customer, [Item("SKU-X", "Thing", amount, 1)])


def test_exactly_at_threshold_ships_free():
    totals = compute_totals(order_worth(50.00))
    assert totals["shipping"] == 0.0
    assert totals["total"] == pytest.approx(53.50, abs=0.005)
    assert qualifies_for_free_shipping(50.00) is True


def test_just_below_threshold_is_still_charged():
    totals = compute_totals(order_worth(49.00))
    assert totals["shipping"] == 5.40
    assert totals["total"] == pytest.approx(57.83, abs=0.005)


def test_well_above_threshold_still_ships_free():
    totals = compute_totals(order_worth(51.00))
    assert totals["shipping"] == 0.0
    assert totals["total"] == pytest.approx(54.57, abs=0.005)


def test_tax_is_charged_on_goods_not_on_shipping():
    totals = compute_totals(order_worth(49.00))
    assert totals["tax"] == pytest.approx(3.43, abs=0.005)


def test_discount_still_applies_before_shipping_is_decided():
    totals = compute_totals(order_worth(60.00, tier="gold"))
    assert totals["discount"] == pytest.approx(6.00, abs=0.005)
    assert totals["goods"] == pytest.approx(54.00, abs=0.005)
    assert totals["shipping"] == 0.0
    assert totals["total"] == pytest.approx(57.78, abs=0.005)


def test_discount_dropping_below_threshold_reinstates_shipping():
    totals = compute_totals(order_worth(52.00, tier="gold"))
    assert totals["goods"] == pytest.approx(46.80, abs=0.005)
    assert totals["shipping"] == 5.40
    assert totals["total"] == pytest.approx(55.48, abs=0.005)


def test_small_order_behaviour_survives():
    customer = Customer("C-2", "Test Buyer", tier="standard")
    order = Order("ORD-2", customer, [Item("SKU-300", "Pen pack", 4.00, 2)])
    totals = compute_totals(order)
    assert totals["subtotal"] == pytest.approx(8.00, abs=0.005)
    assert totals["shipping"] == 5.40
    assert totals["total"] == pytest.approx(13.96, abs=0.005)


def test_validation_and_formatting_survive():
    with pytest.raises(ValueError):
        validate_order(Order("ORD-EMPTY", Customer("C", "N"), []))
    assert format_money(53.5) == "$53.50"


def test_main_prints_the_corrected_total():
    proc = subprocess.run(
        [sys.executable, "main.py"], capture_output=True, text=True
    )
    assert proc.returncode == 0, proc.stderr
    assert "53.50" in proc.stdout
    assert "58.90" not in proc.stdout
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -E '^E |assert|Error' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE '[0-9]+ passed' \
  && pass "free-shipping boundary fixed with no collateral damage" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
