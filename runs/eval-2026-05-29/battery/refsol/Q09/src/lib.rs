//! Run-length encoding of lowercase-letter strings.
//!
//! Each maximal run of a character `c` repeated `n` times is written as the
//! character followed by the decimal count, e.g. "aaabb" <-> "a3b2".

/// Encode a string of lowercase letters using run-length encoding.
pub fn rle_encode(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let mut count = 1usize;
        while chars.peek() == Some(&c) {
            chars.next();
            count += 1;
        }
        out.push(c);
        out.push_str(&count.to_string());
    }
    out
}

/// Decode a run-length-encoded string back to its original form.
pub fn rle_decode(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        let mut digits = String::new();
        while let Some(&d) = chars.peek() {
            if d.is_ascii_digit() {
                digits.push(d);
                chars.next();
            } else {
                break;
            }
        }
        let count: usize = digits.parse().unwrap_or(1);
        for _ in 0..count {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_small() {
        assert_eq!(rle_encode("aaabb"), "a3b2");
    }

    #[test]
    fn decodes_small() {
        assert_eq!(rle_decode("a3b2"), "aaabb");
    }
}
