#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: the boundary trap is multi-digit run counts (>9). A
# decoder that reads a single digit, or an encoder that can't emit "a12", breaks
# the round-trip. Also singletons ("a" -> "a1") and the empty string.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q09::{rle_decode, rle_encode};
#[test] fn encode_basic() { assert_eq!(rle_encode("aaabb"), "a3b2"); }
#[test] fn decode_basic() { assert_eq!(rle_decode("a3b2"), "aaabb"); }
#[test] fn singletons() { assert_eq!(rle_encode("abc"), "a1b1c1"); }
#[test] fn encode_multi_digit() { assert_eq!(rle_encode(&"a".repeat(12)), "a12"); }
#[test] fn decode_multi_digit() { assert_eq!(rle_decode("a12"), "a".repeat(12)); }
#[test] fn round_trip_long_run() {
    let s = format!("{}{}{}", "x".repeat(15), "y".repeat(3), "z".repeat(100));
    assert_eq!(rle_decode(&rle_encode(&s)), s);
}
#[test] fn empty() { assert_eq!(rle_encode(""), ""); assert_eq!(rle_decode(""), ""); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all hidden RLE tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
