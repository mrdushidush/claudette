#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: order-of-a preserved, de-duplicated, no-common and empty
# cases, plus a large disjoint input that only a ~linear (Set-based) solution finishes
# quickly. The nested-includes fixture takes several seconds here.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { intersection } from "./solution.mjs";

assert.deepEqual(intersection([1, 2, 3, 4], [2, 4, 6]), [2, 4], "basic");
assert.deepEqual(intersection([4, 3, 2, 1], [1, 2]), [2, 1], "order of a");
assert.deepEqual(intersection([1, 1, 2, 2], [1, 2]), [1, 2], "unique");
assert.deepEqual(intersection([1, 2], [3, 4]), [], "no common");
assert.deepEqual(intersection([], [1]), [], "empty");

const n = 80000;
const a = Array.from({ length: n }, (_, i) => i);
const b = Array.from({ length: n }, (_, i) => i + n); // disjoint -> full scan, result []
const t = performance.now();
const got = intersection(a, b);
const elapsed = performance.now() - t;
assert.strictEqual(got.length, 0, "disjoint -> empty");
assert.ok(elapsed < 1500, `too slow: ${elapsed.toFixed(0)}ms (needs a linear approach)`);
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|too slow|Error' | head -4 | tr '\n' ' ')"
fi
pass "correct and fast on large input"
