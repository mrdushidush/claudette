"""The order pricing pipeline."""

from app.discount import discount_for
from app.pricing import subtotal
from app.shipping import shipping_fee
from app.tax import tax_for
from app.validation import validate_order


def compute_totals(order):
    """Price an order.

    Discount comes off the subtotal; shipping is decided on the discounted
    goods amount; tax is charged on the goods only, never on shipping.
    """
    validate_order(order)
    goods_before_discount = subtotal(order.items)
    discount = discount_for(order.customer, goods_before_discount)
    goods = round(goods_before_discount - discount, 2)
    shipping = shipping_fee(goods)
    tax = tax_for(goods)
    total = round(goods + shipping + tax, 2)
    return {
        "subtotal": goods_before_discount,
        "discount": discount,
        "goods": goods,
        "shipping": shipping,
        "tax": tax,
        "total": total,
    }
