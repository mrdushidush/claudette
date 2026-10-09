"""Line and order subtotals."""


def line_total(item):
    """Price for one order line, before any discount."""
    return round(item.unit_price * item.quantity, 2)


def subtotal(items):
    """Sum of every line, before any discount."""
    return round(sum(line_total(item) for item in items), 2)


def line_breakdown(items):
    return [(item.name, item.quantity, line_total(item)) for item in items]
