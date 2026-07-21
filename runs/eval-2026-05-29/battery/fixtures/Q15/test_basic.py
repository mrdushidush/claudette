from solution import retry


def test_returns_result_on_success():
    assert retry(lambda: 42, 3) == 42
