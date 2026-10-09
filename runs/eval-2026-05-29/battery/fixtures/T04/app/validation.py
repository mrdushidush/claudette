"""Order validation, run before any pricing happens."""


def validate_order(order):
    """Raise ValueError if the order could not sensibly be priced."""
    if not order.items:
        raise ValueError("order has no items")
    for item in order.items:
        if item.quantity <= 0:
            raise ValueError(f"item {item.sku} has a non-positive quantity")
        if item.unit_price < 0:
            raise ValueError(f"item {item.sku} has a negative price")
    return True
