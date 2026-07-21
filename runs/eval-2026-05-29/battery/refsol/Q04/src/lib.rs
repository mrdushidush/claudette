//! Parse a comma-separated list of integers, e.g. "3, -1, 42".

pub fn parse_int_list(s: &str) -> Vec<i64> {
    s.split(',')
        .map(str::trim)
        .filter(|tok| !tok.is_empty())
        .filter_map(|tok| tok.parse::<i64>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_positive_list() {
        assert_eq!(parse_int_list("1,2,3"), vec![1, 2, 3]);
    }
}
