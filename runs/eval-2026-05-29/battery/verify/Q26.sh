#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: key-order independence, NaN==NaN but NaN!=null, an
# undefined property vs a missing one, nested structures, array/object distinction.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { deepEqual } from "./solution.mjs";

assert.strictEqual(deepEqual({ a: 1, b: 2 }, { b: 2, a: 1 }), true, "key order");
assert.strictEqual(deepEqual({ x: { y: [1, 2] } }, { x: { y: [1, 2] } }), true, "nested");
assert.strictEqual(deepEqual(NaN, NaN), true, "NaN equals NaN");
assert.strictEqual(deepEqual(NaN, null), false, "NaN vs null");
assert.strictEqual(deepEqual({ a: undefined }, {}), false, "undefined prop vs missing");
assert.strictEqual(deepEqual({ a: 1 }, { a: 2 }), false, "different values");
assert.strictEqual(deepEqual([1, 2], [1, 2, 3]), false, "different length");
assert.strictEqual(deepEqual([], {}), false, "array vs object");
assert.strictEqual(deepEqual({ a: 1 }, { a: 1, b: 2 }), false, "extra key");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|order|NaN|prop' | head -4 | tr '\n' ' ')"
fi
pass "all hidden deepEqual tests passed"
