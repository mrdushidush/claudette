//! Detect near-by duplicate values.

use std::collections::HashSet;

/// Return true if there are two equal values in `nums` whose indices are at
/// most `k` apart. Runs in O(n) using a sliding window of the last `k` values.
pub fn has_duplicate_within_k(nums: &[i64], k: usize) -> bool {
    if k == 0 {
        return false;
    }
    let mut window: HashSet<i64> = HashSet::new();
    for (i, &x) in nums.iter().enumerate() {
        if i > k {
            window.remove(&nums[i - k - 1]);
        }
        if !window.insert(x) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_nearby_duplicate() {
        assert!(has_duplicate_within_k(&[1, 2, 3, 1], 3));
    }
}
