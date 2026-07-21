//! Run-length encoding of lowercase-letter strings.
//!
//! Each maximal run of a character `c` repeated `n` times is written as the
//! character followed by the decimal count, e.g. "aaabb" <-> "a3b2".

/// Encode a string of lowercase letters using run-length encoding.
pub fn rle_encode(_s: &str) -> String {
    // TODO: implement
    String::new()
}

/// Decode a run-length-encoded string back to its original form.
pub fn rle_decode(_s: &str) -> String {
    // TODO: implement
    String::new()
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
