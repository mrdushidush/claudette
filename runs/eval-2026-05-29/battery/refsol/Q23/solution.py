def parse_ranges(s):
    """Parse a string like "1-3,5,8-10" into a sorted list of unique integers.

    Ranges are inclusive of both ends; surrounding whitespace is ignored.
    """
    values = set()
    for part in s.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part:
            a, b = part.split("-")
            for n in range(int(a.strip()), int(b.strip()) + 1):
                values.add(n)
        else:
            values.add(int(part))
    return sorted(values)
