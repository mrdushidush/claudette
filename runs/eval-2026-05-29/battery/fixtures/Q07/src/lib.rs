//! Count word frequencies in a piece of text.

use std::collections::HashMap;

/// Return each word paired with how many times it appears, most frequent first.
pub fn word_frequencies(text: &str) -> Vec<(String, usize)> {
    let mut counts = HashMap::new();
    for w in text.split(' ') {
        *counts.entry(w.to_string()).or_insert(0) += 1;
    }
    counts.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_repeats() {
        let freqs = word_frequencies("apple apple banana");
        let apple = freqs.iter().find(|(w, _)| w == "apple").map(|(_, c)| *c);
        assert_eq!(apple, Some(2));
    }
}
