from invoice import invoice_total


def test_two_round_items():
    # 10% tax on two 100c items -> 220c, no rounding drift here.
    assert invoice_total([100, 100], 10) == 220
