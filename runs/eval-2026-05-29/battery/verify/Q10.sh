#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: octet range (>255), leading zeros (non-canonical),
# wrong octet count, empty octets, non-digits, and surrounding whitespace.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q10::is_valid_ipv4;
#[test] fn valid_addresses() {
    assert!(is_valid_ipv4("192.168.0.1"));
    assert!(is_valid_ipv4("0.0.0.0"));
    assert!(is_valid_ipv4("255.255.255.255"));
}
#[test] fn octet_out_of_range() { assert!(!is_valid_ipv4("256.1.1.1")); assert!(!is_valid_ipv4("1.2.3.300")); }
#[test] fn leading_zeros_rejected() { assert!(!is_valid_ipv4("01.2.3.4")); assert!(!is_valid_ipv4("1.2.3.04")); }
#[test] fn zero_octet_ok() { assert!(is_valid_ipv4("10.0.0.1")); }
#[test] fn wrong_octet_count() { assert!(!is_valid_ipv4("1.2.3")); assert!(!is_valid_ipv4("1.2.3.4.5")); }
#[test] fn empty_octet() { assert!(!is_valid_ipv4("1.2.3.")); assert!(!is_valid_ipv4("1..3.4")); }
#[test] fn non_digits_and_space() { assert!(!is_valid_ipv4("1.2.3.a")); assert!(!is_valid_ipv4(" 1.2.3.4")); assert!(!is_valid_ipv4("1.2.3.4 ")); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all hidden ipv4 tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
