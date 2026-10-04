#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests (trap-dense): numeric (not lexicographic) comparison,
# zero-padding for different lengths, equality, and both directions.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q56::compare_versions;
use std::cmp::Ordering;

#[test] fn numeric_not_lexicographic() {
    assert_eq!(compare_versions("1.2.0", "1.10.0"), Ordering::Less);
    assert_eq!(compare_versions("1.10.0", "1.2.0"), Ordering::Greater);
}
#[test] fn equal_versions() {
    assert_eq!(compare_versions("1.2.3", "1.2.3"), Ordering::Equal);
}
#[test] fn shorter_is_zero_padded() {
    assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
    assert_eq!(compare_versions("1.2.0", "1.2"), Ordering::Equal);
}
#[test] fn extra_component_is_greater() {
    assert_eq!(compare_versions("1.0.1", "1.0"), Ordering::Greater);
    assert_eq!(compare_versions("1.0", "1.0.1"), Ordering::Less);
}
#[test] fn major_version() {
    assert_eq!(compare_versions("2.0", "1.9"), Ordering::Greater);
}
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all hidden compare_versions tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
