#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests (trap-dense): quoted commas, escaped quotes, quoted NEWLINES
# (a field spanning lines), CRLF line endings, a trailing newline not making an extra
# row, a trailing empty field, and empty input.
cat > hidden_gate.mjs <<'JS'
import assert from "node:assert/strict";
import { parseCsv } from "./solution.mjs";

assert.deepEqual(parseCsv("a,b,c"), [["a", "b", "c"]], "simple");
assert.deepEqual(parseCsv("a,b\nc,d"), [["a", "b"], ["c", "d"]], "two rows");
assert.deepEqual(parseCsv('a,"b,c",d'), [["a", "b,c", "d"]], "quoted comma");
assert.deepEqual(parseCsv('"he said ""hi"""'), [['he said "hi"']], "escaped quote");
assert.deepEqual(parseCsv('a,"x\ny",b'), [["a", "x\ny", "b"]], "quoted newline");
assert.deepEqual(parseCsv("a,b\r\nc,d"), [["a", "b"], ["c", "d"]], "CRLF");
assert.deepEqual(parseCsv("a,b\nc,d\n"), [["a", "b"], ["c", "d"]], "trailing newline");
assert.deepEqual(parseCsv("a,b,"), [["a", "b", ""]], "trailing empty field");
assert.deepEqual(parseCsv(""), [], "empty input");
console.log("OK");
JS

out=$(node hidden_gate.mjs 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|newline|CRLF' | head -4 | tr '\n' ' ')"
fi
pass "all hidden parseCsv tests passed"
