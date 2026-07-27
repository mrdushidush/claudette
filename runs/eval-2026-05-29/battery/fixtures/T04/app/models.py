"""Plain data holders used across the package."""

from dataclasses import dataclass, field


@dataclass(frozen=True)
class Item:
    sku: str
    name: str
    unit_price: float
    quantity: int = 1


@dataclass
class Customer:
    customer_id: str
    name: str
    tier: str = "standard"


@dataclass
class Order:
    order_id: str
    customer: Customer
    items: list = field(default_factory=list)
