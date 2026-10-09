from solution import paginate


def test_partial_last_page():
    assert paginate([1, 2, 3], 2) == [[1, 2], [3]]
