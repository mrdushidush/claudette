//! Parse simple `key=value` config text into a map.

use std::collections::HashMap;

/// Parse `key=value` lines into a map.
///
/// FIXME: this panics on malformed input; callers want a Result they can handle.
pub fn parse_kv(input: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in input.lines() {
        let (k, v) = line.split_once('=').unwrap();
        m.insert(k.trim().to_string(), v.trim().to_string());
    }
    m
}
