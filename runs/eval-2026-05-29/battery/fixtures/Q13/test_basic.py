from solution import parse_csv_line


def test_plain_fields():
    assert parse_csv_line("a,b,c") == ["a", "b", "c"]
