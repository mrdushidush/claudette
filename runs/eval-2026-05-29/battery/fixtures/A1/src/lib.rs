/// Returns the arithmetic mean of the given values.
pub fn average(xs: &[i64]) -> f64 {
    let total: i64 = xs.iter().sum();
    // BUG: integer division happens before the cast to f64.
    (total / xs.len() as i64) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_average() {
        assert_eq!(average(&[1, 2, 3, 4]), 2.5);
    }
}
