//! Parse simple `key=value` config text into a map.

use std::collections::HashMap;

/// Parse `key=value` lines into a map, returning an error on any malformed line.
pub fn parse_kv(input: &str) -> Result<HashMap<String, String>, String> {
    let mut m = HashMap::new();
    for line in input.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match line.split_once('=') {
            Some((k, v)) => {
                m.insert(k.trim().to_string(), v.trim().to_string());
            }
            None => return Err(format!("malformed line (no '='): {line:?}")),
        }
    }
    Ok(m)
}
