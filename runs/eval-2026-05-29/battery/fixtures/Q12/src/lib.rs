//! Detect near-by duplicate values.

/// Return true if there are two equal values in `nums` whose indices are at
/// most `k` apart.
pub fn has_duplicate_within_k(nums: &[i64], k: usize) -> bool {
    for i in 0..nums.len() {
        let end = (i + k).min(nums.len().saturating_sub(1));
        for j in (i + 1)..=end {
            if nums[i] == nums[j] {
                return true;
            }
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
