#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests (trap-dense): overlap merge, TOUCHING merge (endpoint),
# unsorted input, fully-contained interval, disjoint set sorted, empty, single.
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { mergeIntervals } from "./solution.ts";

assert.deepEqual(mergeIntervals([[1, 3], [2, 4]]), [[1, 4]], "overlap");
assert.deepEqual(mergeIntervals([[1, 2], [2, 3]]), [[1, 3]], "touching merges");
assert.deepEqual(mergeIntervals([[3, 4], [1, 2]]), [[1, 2], [3, 4]], "unsorted input");
assert.deepEqual(mergeIntervals([[1, 5], [2, 3]]), [[1, 5]], "contained");
assert.deepEqual(mergeIntervals([[1, 2], [5, 6], [3, 4]]), [[1, 2], [3, 4], [5, 6]], "disjoint sorted");
assert.deepEqual(mergeIntervals([]), [], "empty");
assert.deepEqual(mergeIntervals([[7, 9]]), [[7, 9]], "single");
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|touching|unsorted' | head -4 | tr '\n' ' ')"
fi
pass "all hidden mergeIntervals tests passed"
