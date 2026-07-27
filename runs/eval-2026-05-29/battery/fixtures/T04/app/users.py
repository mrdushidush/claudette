"""Customer tier helpers."""

from app.config import TIER_DISCOUNTS


def tier_of(customer):
    """The customer's tier, falling back to standard for unknown tiers."""
    if customer.tier in TIER_DISCOUNTS:
        return customer.tier
    return "standard"


def is_eligible_for_discount(customer):
    return TIER_DISCOUNTS[tier_of(customer)] > 0


def display_name(customer):
    return f"{customer.name} ({tier_of(customer)})"
