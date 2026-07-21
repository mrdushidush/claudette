import assert from "node:assert/strict";
import { deepEqual } from "./solution.mjs";

assert.strictEqual(deepEqual({ a: 1 }, { a: 1 }), true);
assert.strictEqual(deepEqual({ a: 1 }, { a: 2 }), false);
console.log("basic OK");
