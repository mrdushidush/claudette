from solution import has_pair_with_sum


def test_finds_a_pair():
    assert has_pair_with_sum([1, 2, 3, 9], 12) is True
