from app.models import Customer, Item, Order
from app.orders import compute_totals


def test_small_order_is_charged_shipping():
    customer = Customer("C-2", "Test Buyer", tier="standard")
    order = Order("ORD-2", customer, [Item("SKU-300", "Pen pack", 4.00, 2)])
    totals = compute_totals(order)
    assert totals["subtotal"] == 8.00
    assert totals["shipping"] == 5.40
    assert totals["tax"] == 0.56
    assert totals["total"] == 13.96
