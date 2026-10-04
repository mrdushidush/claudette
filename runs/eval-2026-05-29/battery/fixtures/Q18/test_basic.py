from solution import add_item


def test_appends_to_given_bucket():
    assert add_item(3, [1, 2]) == [1, 2, 3]
