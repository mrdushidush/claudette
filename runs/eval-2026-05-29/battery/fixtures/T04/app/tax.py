"""Sales tax."""

from app.config import TAX_RATE


def tax_for(taxable_amount):
    """Tax is charged on goods after discount, and never on shipping."""
    return round(taxable_amount * TAX_RATE, 2)
