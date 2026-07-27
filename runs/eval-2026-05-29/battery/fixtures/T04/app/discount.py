"""Tier-based discounts."""

from app.config import TIER_DISCOUNTS
from app.users import tier_of


def discount_rate(customer):
    return TIER_DISCOUNTS[tier_of(customer)]


def discount_for(customer, subtotal_amount):
    """The amount to take off the subtotal for this customer's tier."""
    return round(subtotal_amount * discount_rate(customer), 2)
