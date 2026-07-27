#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# TIER 2 — axis: trap density.
# Four INDEPENDENT cleaning rules, all of which must hold at once:
#   (1) surrounding whitespace trimmed
#   (2) lowercased, and dedupe is therefore case-insensitive
#   (3) empty and whitespace-only tags dropped
#   (4) first-appearance order preserved
# Order preservation rules out the set() one-liner. Case-folding before dedupe
# rules out lowercasing as a final pass over an already-deduped list. Each rule
# is individually fair and stated in the prompt; the difficulty is that a
# partial implementation satisfies most of them and still fails.
cat > hidden_gate_test.py <<'PY'
from solution import normalize_tags


def test_trims_surrounding_whitespace():
    assert normalize_tags("  alpha ,\tbeta  ") == ["alpha", "beta"]


def test_lowercases_and_dedupes_case_insensitively():
    assert normalize_tags("Alpha,alpha,ALPHA") == ["alpha"]
    assert normalize_tags("Alpha,Beta,ALPHA") == ["alpha", "beta"]


def test_drops_empty_and_whitespace_only_tags():
    assert normalize_tags("a,,b") == ["a", "b"]
    assert normalize_tags("a, ,b") == ["a", "b"]
    assert normalize_tags("") == []
    assert normalize_tags(",,,") == []


def test_preserves_first_appearance_order():
    assert normalize_tags("beta,alpha,beta") == ["beta", "alpha"]
    assert normalize_tags("z,y,x") == ["z", "y", "x"]


def test_all_rules_at_once():
    assert normalize_tags(" Beta , alpha,, BETA ,Alpha ") == ["beta", "alpha"]


def test_visible_behaviour_survives():
    assert normalize_tags("alpha,beta") == ["alpha", "beta"]
PY

out=$(python -m pytest -q hidden_gate_test.py 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -E '^E |assert|Error' | head -4 | tr '\n' ' ')"
fi
echo "$out" | grep -qE '[0-9]+ passed' \
  && pass "all four tag-normalisation rules hold together" \
  || fail "no green result: $(echo "$out" | tail -3 | tr '\n' ' ')"
