/// Adds two integers.
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Sums every element of the slice by repeatedly applying `add`.
pub fn add_all(xs: &[i64]) -> i64 {
    xs.iter().fold(0, |acc, &x| add(acc, x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5);
    }

    #[test]
    fn test_add_all() {
        assert_eq!(add_all(&[1, 2, 3, 4]), 10);
    }
}
