//! Parse a comma-separated list of integers, e.g. "3, -1, 42".

pub fn parse_int_list(s: &str) -> Vec<i64> {
    // Scan out each number one digit char at a time.
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_ascii_digit() {
            cur.push(ch);
        } else if ch == ',' {
            if !cur.is_empty() {
                out.push(cur.parse::<i64>().unwrap());
                cur.clear();
            }
        }
        // everything else (spaces, etc.) is ignored
    }
    if !cur.is_empty() {
        out.push(cur.parse::<i64>().unwrap());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_positive_list() {
        assert_eq!(parse_int_list("1,2,3"), vec![1, 2, 3]);
    }
}
