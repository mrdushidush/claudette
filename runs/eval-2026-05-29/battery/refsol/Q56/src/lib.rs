//! Compare two dotted version strings numerically.

use std::cmp::Ordering;

/// Compare two version strings like "1.2.0" and "1.10.0".
///
/// Components are compared numerically, and a shorter version is treated as if
/// padded with zeros (so "1.2" equals "1.2.0").
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let mut ai = a.split('.');
    let mut bi = b.split('.');
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (x, y) => {
                let xv: u64 = x.unwrap_or("0").parse().unwrap_or(0);
                let yv: u64 = y.unwrap_or("0").parse().unwrap_or(0);
                match xv.cmp(&yv) {
                    Ordering::Equal => continue,
                    other => return other,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch_is_greater() {
        assert_eq!(compare_versions("1.0.1", "1.0.0"), Ordering::Greater);
    }
}
