"""Tunable business constants."""

CURRENCY = "USD"

# Sales tax charged on goods (not on shipping).
TAX_RATE = 0.07

# Flat shipping fee applied to orders below the free-shipping threshold.
SHIPPING_FEE = 5.40
FREE_SHIPPING_THRESHOLD = 50.00

# Discount rate by customer tier.
TIER_DISCOUNTS = {
    "standard": 0.00,
    "silver": 0.05,
    "gold": 0.10,
}
