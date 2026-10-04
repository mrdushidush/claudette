def keep_present(values):
    """Return a new list containing the values that are present (not None),
    in their original order."""
    return [v for v in values if v is not None]
