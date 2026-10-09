#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: signature must become Result, malformed lines -> Err,
# and the subtle edge — blank/whitespace-only lines are SKIPPED, not treated as
# malformed (a naive `split_once('=').ok_or(..)?` per line would wrongly error).
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q05::parse_kv;
use std::collections::HashMap;
fn m(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}
#[test] fn basic_ok() { assert_eq!(parse_kv("a=1\nb=2"), Ok(m(&[("a","1"),("b","2")]))); }
#[test] fn trims_whitespace() { assert_eq!(parse_kv("  x  =  hello "), Ok(m(&[("x","hello")]))); }
#[test] fn blank_lines_skipped() { assert_eq!(parse_kv("a=1\n\n   \nb=2"), Ok(m(&[("a","1"),("b","2")]))); }
#[test] fn malformed_is_err() { assert!(parse_kv("a=1\nnope").is_err()); }
#[test] fn empty_value_ok() { assert_eq!(parse_kv("k="), Ok(m(&[("k","")]))); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|mismatched|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all 5 hidden parse_kv tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
