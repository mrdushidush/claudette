def has_pair_with_sum(nums, target):
    """Return True if any two elements at distinct positions sum to `target`."""
    seen = set()
    for x in nums:
        if target - x in seen:
            return True
        seen.add(x)
    return False
