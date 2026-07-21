#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: the char-scanning original silently drops the '-' sign.
# A correct rewrite parses whole tokens, so negatives and surrounding whitespace
# both survive; empty tokens are skipped.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q04::parse_int_list;
#[test] fn positives() { assert_eq!(parse_int_list("1,2,3"), vec![1,2,3]); }
#[test] fn negatives_from_example() { assert_eq!(parse_int_list("3, -1, 42"), vec![3,-1,42]); }
#[test] fn whitespace_padded() { assert_eq!(parse_int_list("  10 ,   20  "), vec![10,20]); }
#[test] fn all_negative() { assert_eq!(parse_int_list("-5,-6,-7"), vec![-5,-6,-7]); }
#[test] fn empty_input() { assert_eq!(parse_int_list(""), Vec::<i64>::new()); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all 5 hidden parse_int_list tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
