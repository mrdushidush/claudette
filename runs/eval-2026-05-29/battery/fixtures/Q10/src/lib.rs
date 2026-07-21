//! IPv4 address validation.

/// Return true iff `s` is a valid dotted-quad IPv4 address,
/// e.g. `is_valid_ipv4("192.168.0.1")` is true.
pub fn is_valid_ipv4(_s: &str) -> bool {
    // TODO: implement
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_normal_address() {
        assert!(is_valid_ipv4("192.168.0.1"));
    }
}
