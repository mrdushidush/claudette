//! Count word frequencies in a piece of text.

use std::collections::HashMap;

/// Return each word paired with how many times it appears, most frequent first
/// (ties broken alphabetically). Case-insensitive; words are runs of alphanumerics.
pub fn word_frequencies(text: &str) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            *counts.entry(std::mem::take(&mut current)).or_insert(0) += 1;
        }
    }
    if !current.is_empty() {
        *counts.entry(current).or_insert(0) += 1;
    }

    let mut freqs: Vec<(String, usize)> = counts.into_iter().collect();
    freqs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    freqs
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
