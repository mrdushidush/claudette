#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: deep hit, missing path -> default (no throw), traversal
# through null -> default, array indices, and a present falsy value returned.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { safeGet } from "./solution.mjs";

assert.strictEqual(safeGet({ a: { b: { c: 1 } } }, "a.b.c"), 1, "deep hit");
assert.strictEqual(safeGet({ a: 1 }, "x.y.z", "def"), "def", "missing path");
assert.strictEqual(safeGet({ a: null }, "a.b", "d"), "d", "through null");
assert.strictEqual(safeGet({ a: [{ b: 5 }] }, "a.0.b"), 5, "array index");
assert.strictEqual(safeGet({ a: { b: 0 } }, "a.b", "def"), 0, "present falsy kept");
assert.strictEqual(safeGet({ a: 1 }, "a.b", "d"), "d", "past a primitive");
assert.strictEqual(safeGet(null, "a.b", "d"), "d", "null root -> default");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|TypeError|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden safeGet tests passed"
