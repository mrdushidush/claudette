"""Print the receipt for a sample order."""

from app.models import Customer, Item, Order
from app.orders import compute_totals
from app.receipt import render


def sample_order():
    customer = Customer("C-1", "Dana Piper", tier="standard")
    items = [
        Item("SKU-100", "Notebook", 12.50, 2),
        Item("SKU-200", "Desk lamp", 25.00, 1),
    ]
    return Order("ORD-1", customer, items)


if __name__ == "__main__":
    order = sample_order()
    print(render(order, compute_totals(order)))
