#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: deterministic count-desc / alphabetical ordering,
# case folding, punctuation as separators, and the empty case.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q07::word_frequencies;
#[test] fn most_frequent_first() {
    let f = word_frequencies("the cat sat on the mat the");
    assert_eq!(f[0], ("the".to_string(), 3));
}
#[test] fn case_insensitive() {
    assert_eq!(word_frequencies("The THE the"), vec![("the".to_string(), 3)]);
}
#[test] fn splits_on_punctuation_and_newlines() {
    assert_eq!(word_frequencies("hi,\thi.\nhi!"), vec![("hi".to_string(), 3)]);
}
#[test] fn ties_broken_alphabetically() {
    assert_eq!(
        word_frequencies("cherry banana apple"),
        vec![("apple".to_string(), 1), ("banana".to_string(), 1), ("cherry".to_string(), 1)]
    );
}
#[test] fn empty_text() {
    assert_eq!(word_frequencies(""), Vec::<(String, usize)>::new());
}
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all 5 hidden word_frequencies tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
