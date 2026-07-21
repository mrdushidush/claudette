#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: multi-digit numeric ordering, negatives, decimals, empty,
# and that the input array is not mutated.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { sortNumbers } from "./solution.mjs";

assert.deepEqual(sortNumbers([10, 2, 1]), [1, 2, 10], "multi-digit");
assert.deepEqual(sortNumbers([-1, -10, 5, 2]), [-10, -1, 2, 5], "negatives");
assert.deepEqual(sortNumbers([10.5, 2.5]), [2.5, 10.5], "decimals");
assert.deepEqual(sortNumbers([]), [], "empty");
const input = [3, 1, 2];
sortNumbers(input);
assert.deepEqual(input, [3, 1, 2], "input not mutated");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden sortNumbers tests passed"
