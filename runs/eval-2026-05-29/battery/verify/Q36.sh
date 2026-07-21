#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: nested objects and arrays must be independent after clone.
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { deepClone } from "./solution.ts";

const obj = { a: { b: 1 } };
const c1 = deepClone(obj);
c1.a.b = 99;
assert.strictEqual(obj.a.b, 1, "nested object independence");

const arr = [[1, 2]];
const c2 = deepClone(arr);
c2[0][0] = 99;
assert.strictEqual(arr[0][0], 1, "nested array independence");

assert.deepEqual(deepClone({ x: { y: [1, 2] } }), { x: { y: [1, 2] } }, "equal values");
assert.strictEqual(deepClone(5), 5, "primitive number");
assert.strictEqual(deepClone("s"), "s", "primitive string");

// array of objects: each cloned object must be independent
const aoo = [{ n: 1 }, { n: 2 }];
const caoo = deepClone(aoo);
caoo[0].n = 99;
assert.strictEqual(aoo[0].n, 1, "array-of-objects element independence");
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|independence' | head -4 | tr '\n' ' ')"
fi
pass "all hidden deepClone tests passed"
