from solution import word_wrap


def test_basic_wrap():
    assert word_wrap("the quick brown fox", 9) == "the quick\nbrown fox"
