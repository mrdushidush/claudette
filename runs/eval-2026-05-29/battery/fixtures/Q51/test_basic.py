from solution import parse_duration


def test_hours_and_minutes():
    assert parse_duration("1h30m") == 5400


def test_seconds_only():
    assert parse_duration("90s") == 90
