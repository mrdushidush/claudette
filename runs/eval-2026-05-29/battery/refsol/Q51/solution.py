import re


def parse_duration(s):
    """Parse a duration string like "1h30m" into a total number of seconds.

    Units are h (hours), m (minutes), s (seconds), e.g. "90s" -> 90.
    Raises ValueError if the string is not a valid duration.
    """
    compact = re.sub(r"\s+", "", s.strip())
    if not compact:
        raise ValueError("empty duration")
    tokens = re.findall(r"(\d+)([hms])", compact)
    # Reject stray characters / bare numbers: the tokens must account for the
    # entire (whitespace-stripped) input.
    if not tokens or "".join(f"{n}{u}" for n, u in tokens) != compact:
        raise ValueError(f"invalid duration: {s!r}")
    multiplier = {"h": 3600, "m": 60, "s": 1}
    return sum(int(n) * multiplier[u] for n, u in tokens)
