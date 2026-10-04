#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: depth must be honored — default 1, explicit 2, Infinity,
# and 0 (no flattening).
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { flatten } from "./solution.mjs";

assert.deepEqual(flatten([1, [2, [3]]]), [1, 2, [3]], "default depth 1");
assert.deepEqual(flatten([1, [2, [3, [4]]]], 2), [1, 2, 3, [4]], "depth 2");
assert.deepEqual(flatten([1, [2, [3, [4]]]], Infinity), [1, 2, 3, 4], "infinity");
assert.deepEqual(flatten([1, [2]], 0), [1, [2]], "depth 0");
assert.deepEqual(flatten([]), [], "empty");
assert.deepEqual(flatten([1, "a", [2, "b"]]), [1, "a", 2, "b"], "mixed elements");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|depth' | head -4 | tr '\n' ' ')"
fi
pass "all hidden flatten tests passed"
