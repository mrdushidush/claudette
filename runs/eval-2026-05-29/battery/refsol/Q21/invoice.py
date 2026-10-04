from taxes import with_tax


def invoice_total(item_cents, rate_pct):
    """Total for a list of item prices (in cents) with tax applied once to the
    subtotal, so per-item rounding doesn't accumulate."""
    subtotal = sum(item_cents)
    return with_tax(subtotal, rate_pct)
