from solution import human_readable_size


def test_bytes_and_kb():
    assert human_readable_size(512) == "512 B"
    assert human_readable_size(1536) == "1.5 KB"
