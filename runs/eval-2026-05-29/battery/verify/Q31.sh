#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: correct grouping, insertion order preserved within groups,
# empty input -> {}, and a single-group case.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { groupBy } from "./solution.mjs";

assert.deepEqual(
  groupBy([1, 2, 3, 4, 5], (n) => (n % 2 === 0 ? "even" : "odd")),
  { odd: [1, 3, 5], even: [2, 4] },
  "basic",
);
assert.deepEqual(
  groupBy(["apple", "banana", "cherry", "avocado"], (s) => s[0]),
  { a: ["apple", "avocado"], b: ["banana"], c: ["cherry"] },
  "order preserved",
);
assert.deepEqual(groupBy([], () => "x"), {}, "empty");
assert.deepEqual(groupBy([2, 4, 6], () => "even"), { even: [2, 4, 6] }, "single group");
assert.deepEqual(groupBy([1, 2, 3, 4], (n) => n % 2), { 0: [2, 4], 1: [1, 3] }, "numeric keys");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden groupBy tests passed"
