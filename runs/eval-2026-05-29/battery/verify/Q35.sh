#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: repeated keys -> array, percent/plus decoding, leading '?'
# stripped, bare key -> "", empty -> {}.
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { parseQueryString } from "./solution.ts";

assert.deepEqual(parseQueryString("a=1&b=2"), { a: "1", b: "2" }, "basic");
assert.deepEqual(parseQueryString("a=1&a=2"), { a: ["1", "2"] }, "repeated key");
assert.deepEqual(parseQueryString("q=hello%20world"), { q: "hello world" }, "percent decode");
assert.deepEqual(parseQueryString("q=a+b"), { q: "a b" }, "plus is space");
assert.deepEqual(parseQueryString("?a=1"), { a: "1" }, "leading question mark");
assert.deepEqual(parseQueryString("flag"), { flag: "" }, "bare key");
assert.deepEqual(parseQueryString(""), {}, "empty");
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error' | head -4 | tr '\n' ' ')"
fi
pass "all hidden parseQueryString tests passed"
