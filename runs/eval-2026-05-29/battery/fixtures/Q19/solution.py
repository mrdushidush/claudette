def human_readable_size(n):
    """Format a byte count as a human-friendly string.

    e.g. 512 -> "512 B", 1536 -> "1.5 KB".
    """
    if n < 1024:
        return f"{n} B"
    return f"{n / 1024} KB"
