from decimal import Decimal


def total_cents(prices):
    """Given a list of price strings like "19.99", return the total number of
    cents as an integer, computed exactly (no floating-point error)."""
    total = 0
    for p in prices:
        total += int((Decimal(p) * 100).to_integral_value())
    return total
