from solution import parse_ranges


def test_single_number():
    assert parse_ranges("5") == [5]
