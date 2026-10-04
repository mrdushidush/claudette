from taxes import with_tax


def invoice_total(item_cents, rate_pct):
    """Total for a list of item prices (in cents) with tax applied."""
    return sum(with_tax(c, rate_pct) for c in item_cents)
