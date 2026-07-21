def has_pair_with_sum(nums, target):
    """Return True if any two elements at distinct positions sum to `target`."""
    n = len(nums)
    for i in range(n):
        for j in range(i + 1, n):
            if nums[i] + nums[j] == target:
                return True
    return False
