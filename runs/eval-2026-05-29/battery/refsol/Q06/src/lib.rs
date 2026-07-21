//! Roman numeral formatting.

/// Convert an integer in the range 1..=3999 to its Roman numeral form.
///
/// e.g. `int_to_roman(4)` is "IV", `int_to_roman(58)` is "LVIII".
pub fn int_to_roman(mut n: u32) -> String {
    const TABLE: [(u32, &str); 13] = [
        (1000, "M"), (900, "CM"), (500, "D"), (400, "CD"),
        (100, "C"), (90, "XC"), (50, "L"), (40, "XL"),
        (10, "X"), (9, "IX"), (5, "V"), (4, "IV"), (1, "I"),
    ];
    let mut out = String::new();
    for (value, symbol) in TABLE {
        while n >= value {
            out.push_str(symbol);
            n -= value;
        }
    }
    out
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
