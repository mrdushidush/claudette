from solution import Counter


def test_single_thread():
    c = Counter()
    for _ in range(100):
        c.increment()
    assert c.get() == 100
