"""Shipping charges."""

from app.config import FREE_SHIPPING_THRESHOLD, SHIPPING_FEE


def shipping_fee(amount):
    """Flat fee, except that orders at or above the free-shipping
    threshold ship free."""
    if amount >= FREE_SHIPPING_THRESHOLD:
        return 0.0
    return SHIPPING_FEE


def qualifies_for_free_shipping(amount):
    return shipping_fee(amount) == 0.0
