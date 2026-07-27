"""The product catalogue and stock lookups."""

CATALOG = {
    "SKU-100": {"name": "Notebook", "price": 12.50, "stock": 40},
    "SKU-200": {"name": "Desk lamp", "price": 25.00, "stock": 12},
    "SKU-300": {"name": "Pen pack", "price": 4.00, "stock": 0},
}


def lookup(sku):
    """Return the catalogue entry for a sku, or None if it is unknown."""
    return CATALOG.get(sku)


def price_of(sku):
    entry = lookup(sku)
    if entry is None:
        raise KeyError(f"unknown sku: {sku}")
    return entry["price"]


def is_in_stock(sku):
    entry = lookup(sku)
    return entry is not None and entry["stock"] > 0
