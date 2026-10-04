def total_cents(prices):
    """Given a list of price strings like "19.99", return the total number of
    cents as an integer."""
    total = 0.0
    for p in prices:
        total += float(p)
    return int(total * 100)
