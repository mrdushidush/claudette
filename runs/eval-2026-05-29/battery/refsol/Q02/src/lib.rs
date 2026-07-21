//! Sorted-slice helpers.

/// Return the index at which `target` should be inserted into the already-sorted
/// slice `xs` to keep it sorted (the leftmost such position).
///
/// e.g. inserting 3 into [1, 2, 4, 5] gives index 2.
pub fn lower_bound(xs: &[i32], target: i32) -> usize {
    let (mut lo, mut hi) = (0usize, xs.len());
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if xs[mid] < target {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_in_middle() {
        assert_eq!(lower_bound(&[1, 2, 4, 5], 3), 2);
    }
}
