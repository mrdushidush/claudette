def parse_ranges(s):
    """Parse a string like "1-3,5,8-10" into a list of integers."""
    result = []
    for part in s.split(","):
        if "-" in part:
            a, b = part.split("-")
            for n in range(int(a), int(b)):
                result.append(n)
        else:
            result.append(int(part))
    return result
