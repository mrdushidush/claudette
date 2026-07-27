def normalize_tags(raw):
    """Turn a comma-separated tag string into a clean list of tags."""
    seen = set()
    out = []
    for part in raw.split(","):
        tag = part.strip().lower()
        if not tag or tag in seen:
            continue
        seen.add(tag)
        out.append(tag)
    return out
