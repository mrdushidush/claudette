"""Human-readable receipt rendering."""

from app.config import CURRENCY
from app.pricing import line_breakdown
from app.users import display_name


def format_money(amount):
    return f"${amount:.2f}"


def render(order, totals):
    lines = [f"Order {order.order_id} for {display_name(order.customer)}"]
    for name, quantity, amount in line_breakdown(order.items):
        lines.append(f"  {quantity} x {name:<12} {format_money(amount)}")
    lines.append(f"  Subtotal      {format_money(totals['subtotal'])}")
    if totals["discount"]:
        lines.append(f"  Discount     -{format_money(totals['discount'])}")
    lines.append(f"  Shipping      {format_money(totals['shipping'])}")
    lines.append(f"  Tax           {format_money(totals['tax'])}")
    lines.append(f"  TOTAL         {format_money(totals['total'])} {CURRENCY}")
    return "\n".join(lines)
