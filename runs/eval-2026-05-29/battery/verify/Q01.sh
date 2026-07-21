#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"   # WORKDIR=$1  TRANSCRIPT=$2 ; fns pass/fail/tc/...
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: the prompt states the goal ("clean slugs for whatever
# titles people throw at it") and shows one example; these grade the edges that
# implies but does not spell out — punctuation stripping, run collapsing, edge
# trimming, and the all-punctuation empty case.
mkdir -p tests
cat > tests/hidden_gate.rs <<'RS'
use q01::slugify;
#[test] fn example_from_prompt() { assert_eq!(slugify("My First Post!"), "my-first-post"); }
#[test] fn collapse_separator_runs() { assert_eq!(slugify("a  --  b"), "a-b"); }
#[test] fn trim_and_strip_punct() { assert_eq!(slugify("  Hello, World!  "), "hello-world"); }
#[test] fn digits_preserved() { assert_eq!(slugify("Rust 101 Rocks"), "rust-101-rocks"); }
#[test] fn all_punctuation_is_empty() { assert_eq!(slugify("!!! ??? ..."), ""); }
#[test] fn already_a_slug_is_stable() { assert_eq!(slugify("already-a-slug"), "already-a-slug"); }
RS

out=$(cargo test --test hidden_gate --quiet 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'panicked|assertion|error\[|FAILED' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE 'test result: ok' \
  && pass "all 6 hidden slug tests passed" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
