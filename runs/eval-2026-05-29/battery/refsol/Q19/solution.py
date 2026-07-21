def human_readable_size(n):
    """Format a byte count as a human-friendly string.

    e.g. 512 -> "512 B", 1536 -> "1.5 KB", 1048576 -> "1.0 MB".
    """
    if n < 1024:
        return f"{n} B"
    value = float(n)
    for unit in ("KB", "MB", "GB", "TB", "PB"):
        value /= 1024
        if value < 1024 or unit == "PB":
            return f"{value:.1f} {unit}"
    return f"{value:.1f} PB"
