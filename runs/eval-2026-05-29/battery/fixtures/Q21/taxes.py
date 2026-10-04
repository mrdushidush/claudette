def with_tax(cents, rate_pct):
    """Add `rate_pct` percent tax to a cents amount, rounded to the nearest cent."""
    return round(cents * (100 + rate_pct) / 100)
