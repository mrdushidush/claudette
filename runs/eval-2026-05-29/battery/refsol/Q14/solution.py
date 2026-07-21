import copy


def deep_merge(a, b):
    """Merge dict b into dict a and return the merged result.

    Values from b take precedence over a. Nested dictionaries are merged
    recursively, and neither input is modified.
    """
    result = copy.deepcopy(a)
    for key, value in b.items():
        if key in result and isinstance(result[key], dict) and isinstance(value, dict):
            result[key] = deep_merge(result[key], value)
        else:
            result[key] = copy.deepcopy(value)
    return result
