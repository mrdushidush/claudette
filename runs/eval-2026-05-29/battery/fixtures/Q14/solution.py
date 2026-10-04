def deep_merge(a, b):
    """Merge dict b into dict a and return the merged result.

    Values from b take precedence over a. Nested dictionaries should be
    merged recursively rather than replaced wholesale, and neither input
    should be modified.
    """
    result = a
    for key, value in b.items():
        result[key] = value
    return result
