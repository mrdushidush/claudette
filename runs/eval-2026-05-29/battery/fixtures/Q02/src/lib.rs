//! Sorted-slice helpers.

/// Return the index at which `target` should be inserted into the already-sorted
/// slice `xs` to keep it sorted (the leftmost such position).
///
/// e.g. inserting 3 into [1, 2, 4, 5] gives index 2.
pub fn lower_bound(_xs: &[i32], _target: i32) -> usize {
    // TODO: implement
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_in_middle() {
        assert_eq!(lower_bound(&[1, 2, 4, 5], 3), 2);
    }
}
