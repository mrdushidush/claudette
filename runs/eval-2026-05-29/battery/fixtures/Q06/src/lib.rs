//! Roman numeral formatting.

/// Convert an integer in the range 1..=3999 to its Roman numeral form.
///
/// e.g. `int_to_roman(4)` is "IV", `int_to_roman(58)` is "LVIII".
pub fn int_to_roman(_n: u32) -> String {
    // TODO: implement
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small() {
        assert_eq!(int_to_roman(4), "IV");
    }

    #[test]
    fn compound() {
        assert_eq!(int_to_roman(58), "LVIII");
    }
}
