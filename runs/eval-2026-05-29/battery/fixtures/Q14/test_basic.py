from solution import deep_merge


def test_merges_disjoint_keys():
    assert deep_merge({"x": 1}, {"y": 2}) == {"x": 1, "y": 2}
