from solution import keep_present


def test_drops_none():
    assert keep_present([1, None, 2]) == [1, 2]
