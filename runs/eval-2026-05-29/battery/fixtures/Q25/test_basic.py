from solution import total_cents


def test_whole_dollars():
    assert total_cents(["1.00", "2.00"]) == 300
