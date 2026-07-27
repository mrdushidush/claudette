from solution import normalize_tags


def test_splits_on_commas():
    assert normalize_tags("alpha,beta") == ["alpha", "beta"]
