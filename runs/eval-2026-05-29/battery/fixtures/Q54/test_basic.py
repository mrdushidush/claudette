from solution import topological_sort


def test_linear_chain():
    assert topological_sort({"b": ["a"], "c": ["b"]}) == ["a", "b", "c"]
