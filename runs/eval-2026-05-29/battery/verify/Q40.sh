#!/usr/bin/env bash
source "$(dirname "$0")/_lib.sh"
cd "$WORKDIR" || fail "cannot cd into workdir"

# Hidden reviewer tests: args passed through; multiple listeners fire in order; off
# removes a specific listener; once fires exactly once; unknown event is a no-op.
cat > hidden_gate.ts <<'TS'
import assert from "node:assert/strict";
import { EventEmitter } from "./solution.ts";

// args + single listener
const a = new EventEmitter();
let got: unknown[] = [];
a.on("e", (...args) => { got = args; });
a.emit("e", 1, "x");
assert.deepEqual(got, [1, "x"], "args passed through");

// multiple listeners in registration order
const b = new EventEmitter();
const order: number[] = [];
b.on("e", () => order.push(1));
b.on("e", () => order.push(2));
b.emit("e");
assert.deepEqual(order, [1, 2], "listeners fire in order");

// off removes a specific listener
const c = new EventEmitter();
let n = 0;
const inc = () => { n += 1; };
c.on("e", inc);
c.off("e", inc);
c.emit("e");
assert.strictEqual(n, 0, "off removes listener");

// once fires exactly once
const d = new EventEmitter();
let m = 0;
d.once("e", () => { m += 1; });
d.emit("e");
d.emit("e");
assert.strictEqual(m, 1, "once fires once");

// unknown event is a no-op (must not throw)
const f = new EventEmitter();
f.emit("nothing");
console.log("OK");
TS

out=$(node hidden_gate.ts 2>&1)
if [ $? -ne 0 ]; then
  fail "hidden tests failed: $(echo "$out" | grep -iE 'AssertionError|Error|once|order' | head -4 | tr '\n' ' ')"
fi
pass "all hidden EventEmitter tests passed"
