#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: left-associative chained subtraction, with precedence
# and mixed operators still intact.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q08::evaluate;
#[test] fn precedence() { assert_eq!(evaluate("2 + 3 * 4"), 14); }
#[test] fn single_subtraction() { assert_eq!(evaluate("10 - 3"), 7); }
#[test] fn chained_subtraction() { assert_eq!(evaluate("10 - 3 - 2"), 5); }
#[test] fn long_subtraction_chain() { assert_eq!(evaluate("100 - 10 - 5 - 1"), 84); }
#[test] fn mixed_precedence_and_sub() { assert_eq!(evaluate("20 - 2 * 3 - 4"), 10); }
#[test] fn all_subtraction() { assert_eq!(evaluate("50 - 20 - 10 - 5"), 15); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all hidden evaluator tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
