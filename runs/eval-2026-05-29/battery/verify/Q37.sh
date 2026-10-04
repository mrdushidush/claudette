#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: descending ranges with negative steps, plus ascending,
# non-dividing, and equal-endpoint cases must all stay correct (end exclusive).
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { range } from "./solution.ts";

assert.deepEqual(range(0, 5), [0, 1, 2, 3, 4], "ascending");
assert.deepEqual(range(0, 10, 2), [0, 2, 4, 6, 8], "step 2");
assert.deepEqual(range(5, 0, -1), [5, 4, 3, 2, 1], "descending");
assert.deepEqual(range(10, 0, -2), [10, 8, 6, 4, 2], "descending step 2");
assert.deepEqual(range(0, 5, 2), [0, 2, 4], "non-dividing stops before end");
assert.deepEqual(range(3, 3), [], "equal endpoints");
assert.deepEqual(range(0, 5, 0), [], "zero step -> empty (no infinite loop)");
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden range tests passed"
