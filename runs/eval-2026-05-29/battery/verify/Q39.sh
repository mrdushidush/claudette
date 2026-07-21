#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: shape/dimensions correct AND rows are independent (writing
# one cell must not bleed into another row).
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { makeGrid } from "./solution.ts";

assert.deepEqual(makeGrid(2, 3), [[0, 0, 0], [0, 0, 0]], "shape");

const g = makeGrid(2, 2);
g[0][0] = 5;
assert.strictEqual(g[1][0], 0, "rows are independent");

const g2 = makeGrid(3, 2);
assert.strictEqual(g2.length, 3, "row count");
assert.strictEqual(g2[0].length, 2, "col count");
g2[1][1] = 9;
assert.strictEqual(g2[0][1], 0, "cell write is isolated");
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|independent' | head -4 | tr '\n' ' ')"
fi
pass "all hidden makeGrid tests passed"
