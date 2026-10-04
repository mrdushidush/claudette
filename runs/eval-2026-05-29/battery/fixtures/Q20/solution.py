def keep_present(values):
    """Return a new list containing the values that are present, in their
    original order."""
    result = []
    for i in range(len(values)):
        v = values[i]
        if v:
            result.append(v)
    return result
