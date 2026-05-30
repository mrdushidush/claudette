/// Returns the arithmetic mean of the given values.
pub fn average(xs: &[i64]) -> f64 {
    let total: i64 = xs.iter().sum();
    total as f64 / xs.len() as f64
}
