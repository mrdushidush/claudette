#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: mean for typical inputs preserved, and empty array -> 0
# (not NaN) — a careless reduce-based rewrite reintroduces NaN.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { average } from "./solution.mjs";

assert.strictEqual(average([2, 4, 6]), 4, "basic");
assert.strictEqual(average([]), 0, "empty -> 0");
assert.strictEqual(average([5]), 5, "single");
assert.strictEqual(average([-2, -4]), -3, "negatives");
assert.strictEqual(average([1, 2]), 1.5, "decimals");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|NaN|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden average tests passed"
