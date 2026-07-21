#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: the subtractive forms (IV, IX, XL, XC, CD, CM) and the
# range boundaries a naive additive implementation gets wrong.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q06::int_to_roman;
#[test] fn one() { assert_eq!(int_to_roman(1), "I"); }
#[test] fn four_ix() { assert_eq!(int_to_roman(4), "IV"); assert_eq!(int_to_roman(9), "IX"); }
#[test] fn fifty_eight() { assert_eq!(int_to_roman(58), "LVIII"); }
#[test] fn forty_ninety() { assert_eq!(int_to_roman(40), "XL"); assert_eq!(int_to_roman(90), "XC"); }
#[test] fn four_nine_hundred() { assert_eq!(int_to_roman(400), "CD"); assert_eq!(int_to_roman(900), "CM"); }
#[test] fn year_1994() { assert_eq!(int_to_roman(1994), "MCMXCIV"); }
#[test] fn max_3999() { assert_eq!(int_to_roman(3999), "MMMCMXCIX"); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all hidden int_to_roman tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
