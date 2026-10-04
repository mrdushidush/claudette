//! Split a bill (in whole cents) evenly between people.

/// Split `total_cents` between `people`, returning each person's share in cents.
/// The shares should add back up to exactly `total_cents`.
pub fn split_bill(total_cents: u64, people: u64) -> Vec<u64> {
    if people == 0 {
        return Vec::new();
    }
    let base = total_cents / people;
    let remainder = total_cents % people;
    (0..people)
        .map(|i| base + if i < remainder { 1 } else { 0 })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_evenly() {
        assert_eq!(split_bill(100, 4), vec![25, 25, 25, 25]);
    }
}
