#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: partial final chunk kept, array shorter than one chunk,
# empty input, exact multiple, one-per-chunk.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { chunk } from "./solution.mjs";

assert.deepEqual(chunk([1, 2, 3, 4, 5], 2), [[1, 2], [3, 4], [5]], "partial last");
assert.deepEqual(chunk([1, 2], 5), [[1, 2]], "shorter than chunk");
assert.deepEqual(chunk([], 3), [], "empty");
assert.deepEqual(chunk([1, 2, 3, 4], 2), [[1, 2], [3, 4]], "exact multiple");
assert.deepEqual(chunk([1, 2, 3], 1), [[1], [2], [3]], "one per chunk");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden chunk tests passed"
