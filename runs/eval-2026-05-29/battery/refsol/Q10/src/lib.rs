//! IPv4 address validation.

/// Return true iff `s` is a valid dotted-quad IPv4 address,
/// e.g. `is_valid_ipv4("192.168.0.1")` is true.
pub fn is_valid_ipv4(s: &str) -> bool {
    let octets: Vec<&str> = s.split('.').collect();
    if octets.len() != 4 {
        return false;
    }
    for octet in octets {
        // must be 1..=3 ASCII digits
        if octet.is_empty() || octet.len() > 3 || !octet.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
        // no leading zeros ("0" alone is fine; "01" is not)
        if octet.len() > 1 && octet.starts_with('0') {
            return false;
        }
        match octet.parse::<u32>() {
            Ok(n) if n <= 255 => {}
            _ => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_normal_address() {
        assert!(is_valid_ipv4("192.168.0.1"));
    }
}
