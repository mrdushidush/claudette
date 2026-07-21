#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: valid parse -> ok; invalid/empty -> {ok:false} without
# throwing, with a string error message.
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { safeJsonParse } from "./solution.ts";

const ok = safeJsonParse<{ a: number }>('{"a":1}');
assert.strictEqual(ok.ok, true, "valid ok");
if (ok.ok) assert.deepEqual(ok.value, { a: 1 }, "valid value");

const arr = safeJsonParse<number[]>("[1,2,3]");
assert.strictEqual(arr.ok, true, "array ok");
if (arr.ok) assert.deepEqual(arr.value, [1, 2, 3], "array value");

const bad = safeJsonParse("{bad");
assert.strictEqual(bad.ok, false, "invalid -> not ok");
if (!bad.ok) assert.strictEqual(typeof bad.error, "string", "error is string");

const empty = safeJsonParse("");
assert.strictEqual(empty.ok, false, "empty -> not ok");
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|SyntaxError|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden safeJsonParse tests passed"
