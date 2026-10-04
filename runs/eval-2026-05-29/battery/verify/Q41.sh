#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: correctness (first repeat by second-occurrence, none, empty,
# immediate) plus a large all-distinct input that only a ~linear approach finishes fast.
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { firstDuplicate } from "./solution.ts";

assert.strictEqual(firstDuplicate([1, 2, 3, 2, 1]), 2, "first repeat");
assert.strictEqual(firstDuplicate([1, 2, 3]), null, "no duplicate");
assert.strictEqual(firstDuplicate([]), null, "empty");
assert.strictEqual(firstDuplicate([5, 5]), 5, "immediate dup");
assert.strictEqual(firstDuplicate([3, 1, 4, 1, 5]), 1, "first by 2nd occurrence");

const n = 80000;
const distinct = Array.from({ length: n }, (_, i) => i);
const t = performance.now();
const got = firstDuplicate(distinct);
const elapsed = performance.now() - t;
assert.strictEqual(got, null, "all distinct -> null");
assert.ok(elapsed < 1500, `too slow: ${elapsed.toFixed(0)}ms (needs a linear approach)`);
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|too slow|Error' | head -4 | tr '\n' ' ')"
fi
pass "correct and fast on large input"
