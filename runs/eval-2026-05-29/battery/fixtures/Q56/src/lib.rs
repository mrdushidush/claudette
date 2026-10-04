//! Compare two dotted version strings numerically.

use std::cmp::Ordering;

/// Compare two version strings like "1.2.0" and "1.10.0".
///
/// Components are compared numerically, and a shorter version is treated as if
/// padded with zeros (so "1.2" equals "1.2.0").
pub fn compare_versions(_a: &str, _b: &str) -> Ordering {
    // TODO: implement
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch_is_greater() {
        assert_eq!(compare_versions("1.0.1", "1.0.0"), Ordering::Greater);
    }
}
